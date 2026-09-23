import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, onCleanup, Show } from "solid-js";
import { credentialExpiryLine, nowMicros } from "/src/format";
import type { ManagementApi } from "/src/management-api/client";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { performManagementCall } from "/src/management-api/outcome";
import { isOAuthProviderRow, providerTypeLabel } from "/src/management-api/vocabulary";
import { hrefs } from "/src/router";
import {
    createPinnedRow,
    createScreenRead,
    isSameProviderRow,
    readAll,
    type ScreenNotice,
    screenFailureNotice,
} from "/src/screens/read";
import { ReadNotice } from "/src/screens/read-notice";
import { EntityUsageCard } from "/src/screens/usage-stats";
import { Button, ButtonLink } from "/src/ui/controls";
import { Badge, EmptyState, Notice, Skeleton } from "/src/ui/feedback";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { ModelList } from "/src/ui/models-table";
import { credentialBadge } from "/src/ui/providers-table";

/**
 * One provider's page: what it is, the models it reports, and what serving
 * clients did with them. Nothing is changed here — the edit page owns every
 * change. Opening a provider with no models starts one discovery from
 * upstream, so an empty list never sits unexplained.
 */
export function ProviderDetailScreen(props: {
    readonly api: ManagementApi;
    readonly providerId: number;
}): JSX.Element {
    const [providers, setProviders] = createSignal<ReadonlyArray<ProviderResponse>>([]);
    const [models, setModels] = createSignal<ReadonlyArray<ModelResponse>>([]);
    const [isDiscovering, setIsDiscovering] = createSignal(false);
    // The discovery's progress and outcome are stated where the action was
    // taken — on the Models card — never in the read-level notice above the
    // page (README §6.4). The read scope stays the read's alone.
    const [discoveryNotice, setDiscoveryNotice] = createSignal<ScreenNotice | null>(null);

    // The provider whose empty list already triggered its one automatic
    // discovery. A plain variable, not a signal: it guards an action, and it
    // lives as long as this screen does.
    let autoDiscoveredFor: number | null = null;

    // Discovery is an action, not the screen read: it carries its own
    // controller, so navigating away mid-discovery cancels it (README §6.4).
    let discoveryController: AbortController | null = null;

    onCleanup(() => {
        discoveryController?.abort();
    });

    // A successful read clears the read-level notice; the discovery's own
    // outcome lives on the Models card and survives reads untouched.
    const read = createScreenRead({
        reReadOn: () => props.providerId,
        load: async (signal) => {
            const [providersResult, modelsResult] = await readAll(
                signal,
                {
                    run: (cycle) => props.api.listProviders(cycle),
                },
                {
                    run: (cycle) =>
                        props.api.listModels(
                            {
                                provider_id: props.providerId,
                            },
                            cycle,
                        ),
                },
            );

            setProviders(providersResult.providers);
            setModels(modelsResult.models);

            if (!providersResult.providers.some((candidate) => candidate.id === props.providerId)) {
                // The subject row is gone: the not-found state takes over,
                // and the discovery outcome stated on its Models card goes
                // with it instead of surviving as a stale banner.
                setDiscoveryNotice(null);
            }

            if (modelsResult.models.length === 0 && autoDiscoveredFor !== props.providerId) {
                // The decision reads the fetched arrays, not the signals: a
                // write from this continuation stages until the next flush,
                // so a read-back here would still see the previous listing.
                const current = providersResult.providers.find(
                    (candidate) => candidate.id === props.providerId,
                );

                if (current !== undefined) {
                    autoDiscoveredFor = props.providerId;
                    void discover(current);
                }
            }
        },
    });

    /** Reads the provider's models from upstream, stating each step where it was taken. */
    const discover = async (current: ProviderResponse): Promise<void> => {
        if (isDiscovering() || read.isDisposed()) {
            return;
        }

        const call = new AbortController();
        discoveryController = call;
        setIsDiscovering(true);
        setDiscoveryNotice({
            tone: "info",
            title: `Discovering models from ${current.name}…`,
            detail: "Asking the upstream what it reports right now.",
        });

        const outcome = await performManagementCall(() =>
            props.api.discoverModels(
                {
                    provider_id: current.id,
                },
                call.signal,
            ),
        );

        setIsDiscovering(false);

        if (outcome.status === "aborted") {
            return;
        }

        if (outcome.status === "failed") {
            setDiscoveryNotice(
                screenFailureNotice(outcome.cause, {
                    action: "discover-models",
                    providerType: current.type,
                }),
            );
            return;
        }

        const rows = outcome.result.models;
        const discovered = rows.filter((row) => row.discovery_state === "discovered").length;
        const vanished = rows.filter((row) => row.discovery_state === "vanished").length;
        const manual = rows.length - discovered - vanished;

        // Discovery never writes `id_override`, so every override in the
        // answer is a pre-existing custom name the refresh kept — a first
        // discovery that inserts only fresh rows keeps none.
        const keptNames = rows.filter((row) => row.id_override !== null).length;

        const facts: string[] = [];

        if (keptNames > 0) {
            facts.push(`${keptNames} custom ${keptNames === 1 ? "name was" : "names were"} kept.`);
        }

        if (manual > 0) {
            facts.push(`${manual} added by hand ${manual === 1 ? "was" : "were"} left untouched.`);
        }

        if (vanished > 0) {
            facts.push(
                `${vanished} ${vanished === 1 ? "model is" : "models are"} now listed as no longer reported.`,
            );
        }

        setDiscoveryNotice({
            tone: "ok",
            title:
                discovered > 0
                    ? `${discovered} ${discovered === 1 ? "model" : "models"} discovered from ${current.name}.`
                    : `Nothing new discovered from ${current.name}.`,
            detail: facts.length > 0 ? facts.join(" ") : null,
        });

        // A discovery answering after the visitor left must not read again.
        if (read.isDisposed()) {
            return;
        }

        read.reload();
    };

    const provider = createPinnedRow(
        () => providers().find((candidate) => candidate.id === props.providerId) ?? null,
        isSameProviderRow,
    );

    const providersById = createMemo(
        () =>
            new Map<number, ProviderResponse>(
                providers().map((row) => [
                    row.id,
                    row,
                ]),
            ),
    );

    return (
        <div class="rise-in flex flex-col gap-5">
            <ReadNotice read={read} />

            <Show when={read.isFirstRead()}>
                <PageHeader
                    title="Provider"
                    backHref={hrefs.providers()}
                    backLabel="Providers"
                />
                <Skeleton class="h-64 w-full rounded-2xl" />
            </Show>

            <Show when={read.hasRead() && provider() === null}>
                <PageHeader
                    title="Provider"
                    backHref={hrefs.providers()}
                    backLabel="Providers"
                />
                <EmptyState
                    title="Provider not found"
                    description="The gateway holds no provider here. It was deleted — on this page, in another session, or by hand. Nothing here can bring it back."
                >
                    <ButtonLink
                        href={hrefs.providers()}
                        tone="primary"
                    >
                        Back to providers
                    </ButtonLink>
                    <ButtonLink href={hrefs.providerNew()}>Create provider</ButtonLink>
                </EmptyState>
            </Show>

            <Show
                when={provider()}
                keyed
            >
                {(row) => (
                    <>
                        <PageHeader
                            title={row.name}
                            description={`${providerTypeLabel(row.type)} · prefix ${row.model_prefix === "" ? "(empty)" : row.model_prefix}`}
                            backHref={hrefs.providers()}
                            backLabel="Providers"
                        >
                            <Badge
                                tone={row.is_enabled ? "ok" : "muted"}
                                label={row.is_enabled ? "Enabled" : "Disabled"}
                            />
                            <Badge
                                tone={credentialBadge(row).tone}
                                label={credentialBadge(row).label}
                            />
                            <ButtonLink href={hrefs.providerEdit(row.id)}>Edit provider</ButtonLink>
                        </PageHeader>

                        <FactsCard provider={row} />

                        <ModelsCard
                            provider={row}
                            models={models()}
                            providersById={providersById()}
                            isDiscovering={isDiscovering()}
                            notice={discoveryNotice()}
                            onDiscover={() => void discover(row)}
                        />

                        <EntityUsageCard
                            api={props.api}
                            filter={{
                                provider_id: row.id,
                            }}
                            subjectKind="provider"
                        />
                    </>
                )}
            </Show>
        </div>
    );
}

/** What this provider is, read-only: every change lives on the edit page. */
function FactsCard(props: { readonly provider: ProviderResponse }): JSX.Element {
    // A static key's row carries the base URL; an OAuth login's row has none.
    const baseUrl = (): string | null =>
        isOAuthProviderRow(props.provider) ? null : props.provider.base_url;

    const credentialLine = (): string | null => {
        if (!isOAuthProviderRow(props.provider) || props.provider.expires_at === null) {
            return null;
        }

        return credentialExpiryLine(props.provider.expires_at, nowMicros());
    };

    return (
        <Card>
            <CardHeader title="Details" />
            <dl class="mt-4 grid gap-x-8 gap-y-3 sm:grid-cols-2">
                <div>
                    <dt class="text-[12.5px] font-medium text-ink-dim">Type</dt>
                    <dd class="mt-1">{providerTypeLabel(props.provider.type)}</dd>
                </div>
                <div>
                    <dt class="text-[12.5px] font-medium text-ink-dim">Model prefix</dt>
                    <dd class="tnum mt-1 break-all">
                        {props.provider.model_prefix === ""
                            ? "(empty)"
                            : props.provider.model_prefix}
                    </dd>
                </div>
                <Show when={baseUrl()}>
                    {(url) => (
                        <div>
                            <dt class="text-[12.5px] font-medium text-ink-dim">
                                Upstream base URL
                            </dt>
                            <dd class="tnum mt-1 break-all">{url()}</dd>
                        </div>
                    )}
                </Show>
                <Show when={credentialLine()}>
                    {(line) => (
                        <div>
                            <dt class="text-[12.5px] font-medium text-ink-dim">Credentials</dt>
                            <dd class="mt-1 text-ink-dim">{line()}</dd>
                        </div>
                    )}
                </Show>
                <div>
                    <dt class="text-[12.5px] font-medium text-ink-dim">Enabled</dt>
                    <dd class="mt-1">{props.provider.is_enabled ? "On" : "Off"}</dd>
                </div>
            </dl>
        </Card>
    );
}

/**
 * The provider's models and the discovery that refreshes them from upstream.
 * The discovery's progress and outcome are stated here, where the action was
 * taken — never at the top of the page, which states read failures only.
 */
function ModelsCard(props: {
    readonly provider: ProviderResponse;
    readonly models: ReadonlyArray<ModelResponse>;
    readonly providersById: ReadonlyMap<number, ProviderResponse>;
    readonly isDiscovering: boolean;
    readonly notice: ScreenNotice | null;
    readonly onDiscover: () => void;
}): JSX.Element {
    return (
        <Card>
            <CardHeader
                title={`Models · ${props.models.length}`}
                description="Discovered from upstream, or added by hand."
            >
                <ButtonLink
                    href={hrefs.modelNew(props.provider.id)}
                    glyph="plus"
                >
                    Add model
                </ButtonLink>
                <Button
                    glyph="refresh"
                    isBusy={props.isDiscovering}
                    onClick={props.onDiscover}
                >
                    {props.isDiscovering ? "Discovering…" : "Discover models"}
                </Button>
            </CardHeader>
            <Show when={props.notice}>
                {(current) => (
                    <div class="mt-4">
                        <Notice
                            tone={current().tone}
                            title={current().title}
                            detail={current().detail}
                            fix={current().fix}
                        />
                    </div>
                )}
            </Show>
            <div class="mt-4">
                <Show
                    when={props.models.length > 0}
                    fallback={
                        <p class="rounded-lg bg-canvas px-3.5 py-3 text-[13.5px] text-ink-dim dark:bg-muted">
                            {props.isDiscovering
                                ? "Asking the upstream what it reports…"
                                : "No models on this provider yet — discover what the provider reports, or add one by hand."}
                        </p>
                    }
                >
                    <ModelList
                        models={props.models}
                        providersById={props.providersById}
                    />
                </Show>
            </div>
        </Card>
    );
}
