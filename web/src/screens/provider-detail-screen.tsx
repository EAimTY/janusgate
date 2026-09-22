import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, createTrackedEffect, onCleanup, Show } from "solid-js";
import { credentialExpiryLine, nowMicros } from "/src/format";
import type { ManagementApi } from "/src/management-api/client";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { performManagementCall } from "/src/management-api/outcome";
import { isOAuthProviderRow, providerTypeLabel } from "/src/management-api/vocabulary";
import { refreshRequest, reportRefresh } from "/src/refresh-state";
import { hrefs } from "/src/router";
import { describeCallFailure } from "/src/screens/failure";
import { EntityUsageCard } from "/src/screens/usage-stats";
import { Button, ButtonLink } from "/src/ui/controls";
import { Badge, EmptyState, Notice, type NoticeTone, Skeleton } from "/src/ui/feedback";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { ModelList } from "/src/ui/models-table";
import { credentialBadge } from "/src/ui/providers-table";

type ScreenNotice = {
    readonly tone: NoticeTone;
    readonly title: string;
    readonly detail?: string | null | undefined;
    readonly fix?: string | null | undefined;
};

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
    const [hasRead, setHasRead] = createSignal(false);
    const [isReading, setIsReading] = createSignal(true);
    const [isDiscovering, setIsDiscovering] = createSignal(false);
    const [notice, setNotice] = createSignal<ScreenNotice | null>(null);

    let controller: AbortController | null = null;
    let discoveryController: AbortController | null = null;
    // Set when the screen goes away: a discovery answering afterwards must
    // neither state anything nor start its follow-up read on a dead screen.
    let isDisposed = false;

    onCleanup(() => {
        isDisposed = true;
        discoveryController?.abort();
    });

    // The provider whose empty list already triggered its one automatic
    // discovery. A plain variable, not a signal: it guards an action, and it
    // lives as long as this screen does.
    let autoDiscoveredFor: number | null = null;

    const read = async (): Promise<void> => {
        controller?.abort();

        const call = new AbortController();
        controller = call;
        setIsReading(true);

        const [providersOutcome, modelsOutcome] = await Promise.all([
            performManagementCall(() => props.api.listProviders(call.signal)),
            performManagementCall(() =>
                props.api.listModels(
                    {
                        provider_id: props.providerId,
                    },
                    call.signal,
                ),
            ),
        ]);

        if (providersOutcome.status === "aborted" || modelsOutcome.status === "aborted") {
            return;
        }

        setIsReading(false);

        const failed =
            providersOutcome.status === "failed"
                ? providersOutcome
                : modelsOutcome.status === "failed"
                  ? modelsOutcome
                  : null;

        if (failed !== null) {
            const diagnosis = describeCallFailure(failed.cause);

            setNotice({
                tone: "bad",
                title: diagnosis.headline,
                detail: diagnosis.detail,
                fix: diagnosis.fix,
            });
            return;
        }

        const freshProviders =
            providersOutcome.status === "ok" ? providersOutcome.result.providers : [];
        const freshModels = modelsOutcome.status === "ok" ? modelsOutcome.result.models : [];
        setProviders(freshProviders);
        setModels(freshModels);
        setHasRead(true);
        reportRefresh();

        if (freshModels.length === 0 && autoDiscoveredFor !== props.providerId) {
            // The decision reads the fetched arrays, not the signals: a write
            // from this continuation stages until the next flush, so a
            // read-back here would still see the previous listing.
            const current = freshProviders.find((candidate) => candidate.id === props.providerId);

            if (current !== undefined) {
                autoDiscoveredFor = props.providerId;
                void discover(current);
            }
        }
    };

    /** Reads the provider's models from upstream, stating each step where it was taken. */
    const discover = async (current: ProviderResponse): Promise<void> => {
        if (isDiscovering() || isDisposed) {
            return;
        }

        const call = new AbortController();
        discoveryController = call;
        setIsDiscovering(true);
        setNotice({
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
            const diagnosis = describeCallFailure(outcome.cause, "discover-models");

            setNotice({
                tone: "bad",
                title: diagnosis.headline,
                detail: diagnosis.detail,
                fix: diagnosis.fix,
            });
            return;
        }

        const rows = outcome.result.models;
        const discovered = rows.filter((row) => row.discovery_state === "discovered").length;
        const vanished = rows.filter((row) => row.discovery_state === "vanished").length;
        const manual = rows.length - discovered - vanished;

        const facts = [
            "Custom names were kept.",
        ];

        if (manual > 0) {
            facts.push(`${manual} added by hand ${manual === 1 ? "was" : "were"} left untouched.`);
        }

        if (vanished > 0) {
            facts.push(
                `${vanished} ${vanished === 1 ? "model is" : "models are"} now listed as no longer reported.`,
            );
        }

        setNotice({
            tone: "ok",
            title:
                discovered > 0
                    ? `${discovered} ${discovered === 1 ? "model" : "models"} discovered from ${current.name}.`
                    : `Nothing new discovered from ${current.name}.`,
            detail: facts.join(" "),
        });

        // A discovery answering after the visitor left must not read again.
        if (isDisposed) {
            return;
        }

        void read();
    };

    createTrackedEffect(() => {
        props.providerId;
        refreshRequest();
        void read();

        return () => controller?.abort();
    });

    const provider = createMemo(
        () => providers().find((candidate) => candidate.id === props.providerId) ?? null,
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
            <Show when={notice()}>
                {(current) => (
                    <Notice
                        tone={current().tone}
                        title={current().title}
                        detail={current().detail}
                        fix={current().fix}
                    />
                )}
            </Show>

            <Show when={isReading() && !hasRead() && notice() === null}>
                <PageHeader
                    title="Provider"
                    backHref={hrefs.providers()}
                    backLabel="Providers"
                />
                <Skeleton class="h-64 w-full rounded-2xl" />
            </Show>

            <Show when={hasRead() && provider() === null}>
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
    const baseUrl = (): string | null => {
        switch (props.provider.type) {
            case "openai_response":
            case "openai_chat_completion":
            case "gemini_generate_content":
                return props.provider.base_url;

            case "codex_oauth":
            case "antigravity_oauth":
                return null;
        }
    };

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

/** The provider's models and the discovery that refreshes them from upstream. */
function ModelsCard(props: {
    readonly provider: ProviderResponse;
    readonly models: ReadonlyArray<ModelResponse>;
    readonly providersById: ReadonlyMap<number, ProviderResponse>;
    readonly isDiscovering: boolean;
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
