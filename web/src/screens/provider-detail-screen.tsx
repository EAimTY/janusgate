import type { JSX } from "@solidjs/web";
import { type Accessor, createMemo, createSignal, onCleanup, Show } from "solid-js";
import { credentialExpiryLine, nowMicros } from "/src/format";
import type { ManagementApi } from "/src/management-api/client";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { performManagementCall } from "/src/management-api/outcome";
import { indexRowsById } from "/src/management-api/serving";
import {
    credentialKindLabel,
    isOAuthProviderRow,
    upstreamFormatLabel,
} from "/src/management-api/vocabulary";
import { PROVIDER_PRESETS, type ProviderPreset } from "/src/presets";
import { hrefs } from "/src/router";
import {
    createPinnedRow,
    createScreenRead,
    isSameProviderRow,
    readAll,
    readModels,
    readProviders,
    type ScreenNotice,
    screenFailureNotice,
} from "/src/screens/read";
import { ScreenFrame } from "/src/screens/screen-frame";
import { EntityUsageCard } from "/src/screens/usage-stats";
import { AppLink } from "/src/ui/app-link";
import { Button, ButtonLink } from "/src/ui/controls";
import { Badge, Notice } from "/src/ui/feedback";
import { Card, CardHeader, Fact, FactList, PageHeader } from "/src/ui/layout";
import { ModelList } from "/src/ui/models-table";
import { credentialBadge } from "/src/ui/providers-table";
import { StatedNotice } from "./read-notice";
import { RowNotFound } from "./row-not-found";

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
        load: async (signal) => {
            const [providersResult, modelsResult] = await readAll(
                signal,
                readProviders(props.api),
                readModels(props.api, {
                    provider_id: props.providerId,
                }),
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
                    void discover(current, modelsResult.models);
                }
            }
        },
    });

    /**
     * Reads the provider's models from upstream, stating each step where it
     * was taken. `before` is the listing the screen held when the discovery
     * started — the baseline the answer is diffed against.
     */
    const discover = async (
        current: ProviderResponse,
        before: ReadonlyArray<ModelResponse>,
    ): Promise<void> => {
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
                    isOAuthProvider: isOAuthProviderRow(current),
                }),
            );
            return;
        }

        const rows = outcome.result.models;

        // The answer carries the provider's complete fresh rows — the rows it
        // already held included (README §7.1 models:discover) — so the
        // headline counts only the `upstream_id`s the pre-discovery listing
        // did not hold. A refresh of an unchanged upstream announces nothing.
        const knownUpstreamIds = new Set(before.map((row) => row.upstream_id));
        const added = rows.filter((row) => !knownUpstreamIds.has(row.upstream_id)).length;
        const vanished = rows.filter((row) => row.discovery_state === "vanished").length;
        const manual = rows.filter((row) => row.discovery_state === "manual").length;

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
                added > 0
                    ? `${added} ${added === 1 ? "model" : "models"} discovered from ${current.name}.`
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

    const providersById = createMemo(() => indexRowsById(providers()));

    return (
        <ScreenFrame
            read={read}
            row={provider}
            firstReadHeader={
                <PageHeader
                    title="Provider"
                    backHref={hrefs.providers()}
                    backLabel="Providers"
                />
            }
            notFound={
                <RowNotFound
                    kind="provider"
                    headerTitle="Provider"
                    createAction={{
                        href: hrefs.providerNew(),
                        label: "Create provider",
                    }}
                />
            }
        >
            {(row) => (
                <>
                    <PageHeader
                        title={row.name}
                        description={`${upstreamFormatLabel(row.format)} · ${credentialKindLabel(row.type)} · prefix ${row.model_prefix === "" ? "(empty)" : row.model_prefix}`}
                        backHref={hrefs.providers()}
                        backLabel="Providers"
                    >
                        <Badge
                            tone={row.is_enabled ? "ok" : "muted"}
                            label={row.is_enabled ? "Enabled" : "Disabled"}
                        />
                        <Show when={credentialBadge(row)}>
                            {(credential) => (
                                <Badge
                                    tone={credential().tone}
                                    label={credential().label}
                                />
                            )}
                        </Show>
                        <ButtonLink href={hrefs.providerEdit(row.id)}>Edit provider</ButtonLink>
                    </PageHeader>

                    {/* A lapsed credential's fix lives one page over, where
                        reauthorization runs — named and linked the way the
                        overview's attention rows name it, not left to a bare
                        badge beside a generic edit button. */}
                    <Show when={lapsedCredentialLabel(row)}>
                        {(label) => (
                            <Notice
                                tone="warn"
                                title={
                                    label() === "Expired"
                                        ? "The OAuth credentials have expired."
                                        : "The OAuth credentials were never fresh."
                                }
                                detail={
                                    <>
                                        {"Reconnect on the "}
                                        <AppLink
                                            href={hrefs.providerEdit(row.id)}
                                            class="font-medium text-accent hover:text-accent-strong"
                                        >
                                            edit page
                                        </AppLink>
                                        {" — its Credentials card replaces them."}
                                    </>
                                }
                            />
                        )}
                    </Show>

                    <FactsCard provider={row} />

                    <ModelsCard
                        provider={row}
                        models={models()}
                        providersById={providersById()}
                        isDiscovering={isDiscovering()}
                        notice={discoveryNotice}
                        onDiscover={() => void discover(row, models())}
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
        </ScreenFrame>
    );
}

/**
 * The credential badge's word when it names a credential the gateway cannot
 * use as stored — `Expired`, or the never-fresh `Stale` — and `null` when it
 * does not: a valid OAuth login needs no reconnect, and a static key, which
 * never expires, earns no credential badge at all. The page's reconnect line
 * gates on exactly this, so the notice and the header badge can never
 * disagree about whether a fix is called for.
 */
function lapsedCredentialLabel(provider: ProviderResponse): "Expired" | "Stale" | null {
    const label = credentialBadge(provider)?.label;

    return label === "Expired" || label === "Stale" ? label : null;
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

    /**
     * The catalog preset the stored definition came from, when its name is a
     * preset id: at creation the operator picked the preset by title, never
     * by the machine name the gateway stores, so the fact leads with that
     * title. A name outside the catalog is a hand-authored definition and
     * states itself alone — no title is ever invented for it.
     */
    const definitionPreset = (): ProviderPreset | null =>
        PROVIDER_PRESETS.find(
            (candidate) => candidate.id === props.provider.provider_definition_name,
        ) ?? null;

    /** The stored definition identity: the gateway's `name` plus its version. */
    const storedDefinition = (): string =>
        `${props.provider.provider_definition_name} v${props.provider.provider_definition_version}`;

    return (
        <Card>
            <CardHeader title="Details" />
            <FactList>
                <Fact
                    label="Type"
                    value={upstreamFormatLabel(props.provider.format)}
                />
                <Fact
                    label="Definition"
                    value={
                        <Show
                            when={definitionPreset()}
                            fallback={storedDefinition()}
                        >
                            {(preset) => (
                                <>
                                    {preset().title}
                                    <span class="mt-0.5 block text-[12.5px] leading-snug text-ink-dim">
                                        {storedDefinition()}
                                    </span>
                                </>
                            )}
                        </Show>
                    }
                    isTabular
                    class="break-all"
                />
                <Fact
                    label="Model prefix"
                    value={
                        props.provider.model_prefix === "" ? "(empty)" : props.provider.model_prefix
                    }
                    isTabular
                    class="break-all"
                />
                <Show when={baseUrl()}>
                    {(url) => (
                        <Fact
                            label="Upstream base URL"
                            value={url()}
                            isTabular
                            class="break-all"
                        />
                    )}
                </Show>
                <Show when={credentialLine()}>
                    {(line) => (
                        <Fact
                            label="Credentials"
                            value={line()}
                            class="text-ink-dim"
                        />
                    )}
                </Show>
                {/* The one enabled pair this page speaks: the Details card's
                    value echoes the header badge's word exactly — the tight
                    list-row "On/Off" never appears on a detail page. */}
                <Fact
                    label="Status"
                    value={props.provider.is_enabled ? "Enabled" : "Disabled"}
                />
            </FactList>
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
    readonly notice: Accessor<ScreenNotice | null>;
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
                    Create model
                </ButtonLink>
                <Button
                    glyph="refresh"
                    isBusy={props.isDiscovering}
                    onClick={props.onDiscover}
                >
                    {props.isDiscovering ? "Discovering…" : "Discover models"}
                </Button>
                {/* The inline list stays unsearchable; the quiet door hands
                    the visitor the searchable models view filtered to this
                    provider — the address the home screen's "Review models"
                    row already speaks. */}
                <ButtonLink
                    href={hrefs.models(props.provider.id)}
                    tone="ghost"
                >
                    All models
                </ButtonLink>
            </CardHeader>
            <StatedNotice
                notice={props.notice}
                class="mt-4"
            />
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
