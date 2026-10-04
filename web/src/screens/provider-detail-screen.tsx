import type { JSX } from "@solidjs/web";
import { type Accessor, createMemo, createSignal, onCleanup, Show } from "solid-js";
import { credentialExpiryLine, nowMicros } from "/src/format";
import { translation } from "/src/i18n";
import { discoveryPendingMessage, reconnectMessage } from "/src/i18n/rich";
import type { ManagementApi } from "/src/management-api/client";
import type {
    ModelResponse,
    ModelRouteResponse,
    ProviderPoolResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import { performManagementCall } from "/src/management-api/outcome";
import { lapsedCredentialStanding } from "/src/management-api/serving";
import {
    CREDENTIAL_KIND,
    isOAuthProviderRow,
    UPSTREAM_FORMAT,
} from "/src/management-api/vocabulary";
import { PROVIDER_PRESETS, type ProviderPreset } from "/src/presets";
import { requestRefresh } from "/src/refresh-state";
import { hrefs } from "/src/router";
import { indexRowsById } from "/src/row-index";
import {
    createPinnedRow,
    createScreenRead,
    isSameProviderRow,
    readOne,
    readPublishedInventory,
    type ScreenNotice,
    screenFailureNotice,
} from "/src/screens/read";
import { ScreenFrame } from "/src/screens/screen-frame";
import { EntityUsageCard } from "/src/screens/usage-stats";
import { Button, ButtonLink } from "/src/ui/controls";
import { Badge, Notice } from "/src/ui/feedback";
import { Card, CardHeader, Fact, FactList, PageHeader } from "/src/ui/layout";
import { ModelList } from "/src/ui/models-table";
import { credentialBadge, oauthNeverFreshSentence } from "/src/ui/providers-table";
import { AccountAvailabilityRow } from "./pool-availability-card";
import { ProviderQuotaCard } from "./provider-quota-card";
import { createProviderQuotaView } from "./quota-view";
import { StatedNotice } from "./read-notice";
import { RowNotFound } from "./row-not-found";
import { createDiscoveryNotice } from "./success-notices";

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
    const [modelRoutes, setModelRoutes] = createSignal<ReadonlyArray<ModelRouteResponse>>([]);
    const [pools, setPools] = createSignal<ReadonlyArray<ProviderPoolResponse>>([]);
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
            const catalog = await readOne(
                signal,
                readPublishedInventory(props.api, props.providerId),
            );

            setProviders(catalog.providers);
            setModels(catalog.models);
            setModelRoutes(catalog.modelRoutes);
            setPools(catalog.pools);

            if (!catalog.providers.some((candidate) => candidate.id === props.providerId)) {
                // The subject row is gone: the not-found state takes over,
                // and the discovery outcome stated on its Models card goes
                // with it instead of surviving as a stale banner.
                setDiscoveryNotice(null);
            }

            if (catalog.models.length === 0 && autoDiscoveredFor !== props.providerId) {
                // The decision reads the fetched arrays, not the signals: a
                // write from this continuation stages until the next flush,
                // so a read-back here would still see the previous listing.
                const current = catalog.providers.find(
                    (candidate) => candidate.id === props.providerId,
                );

                if (current !== undefined) {
                    autoDiscoveredFor = props.providerId;
                    void discover(current, catalog.models);
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
            get title() {
                return discoveryPendingMessage(translation(), current.name);
            },
            get detail() {
                return translation()(($) => $.operationNotices.discoveryQuery);
            },
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

        setDiscoveryNotice(
            createDiscoveryNotice({
                providerName: current.name,
                added,
                vanished,
                manual,
            }),
        );

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
    const quota = createProviderQuotaView({
        onQuotaReadComplete: requestRefresh,
        api: () => props.api,
        provider,
    });

    return (
        <ScreenFrame
            read={read}
            row={provider}
            firstReadHeader={
                <PageHeader
                    title={translation()(($) => $.screens.providerDetailScreen.provider)}
                    backHref={hrefs.providers()}
                    backLabel={translation()(($) => $.additional.providerDetailScreen.providers)}
                />
            }
            notFound={
                <RowNotFound
                    kind="provider"
                    headerTitle={translation()(($) => $.additional.providerDetailScreen.provider)}
                    createAction={{
                        href: hrefs.providerNew(),
                        label: translation()(
                            ($) => $.additional.providerDetailScreen.createProvider,
                        ),
                    }}
                />
            }
        >
            {(row) => (
                <>
                    <PageHeader
                        title={row.name}
                        description={`${UPSTREAM_FORMAT.label(row.format)} · ${CREDENTIAL_KIND.label(row.type)}`}
                        backHref={hrefs.providers()}
                        backLabel={translation()(
                            ($) => $.additional.providerDetailScreen.providers,
                        )}
                    >
                        <Badge
                            tone={row.is_enabled ? "ok" : "muted"}
                            label={
                                row.is_enabled
                                    ? translation()(
                                          ($) => $.additional.providerDetailScreen.enabled,
                                      )
                                    : translation()(
                                          ($) => $.additional.providerDetailScreen.disabled,
                                      )
                            }
                        />
                        <Show when={credentialBadge(row)}>
                            {(credential) => (
                                <Badge
                                    tone={credential().tone}
                                    label={credential().label}
                                />
                            )}
                        </Show>
                        <ButtonLink href={hrefs.providerEdit(row.id)}>
                            {translation()(($) => $.screens.providerDetailScreen.editProvider)}
                        </ButtonLink>
                    </PageHeader>

                    {/* A lapsed credential's fix lives one page over, where
                        reauthorization runs — named and linked the way the
                        overview's attention rows name it, not left to a bare
                        badge beside a generic edit button. The gate is
                        `serving.ts`'s standing, never a badge's rendered word,
                        so a renamed label cannot silently detach the notice.
                        A parked provider defers the way the overview's
                        attention row defers: reconnecting waits for the
                        re-enable the operator chose to defer, so the line
                        says so instead of ordering the reconnect now — while
                        still naming the edit page, where both the re-enable
                        and the reconnect live. */}
                    <Show when={lapsedCredentialStanding(row)}>
                        {(standing) => (
                            <Notice
                                tone="warn"
                                title={
                                    standing() === "expired"
                                        ? translation()(
                                              ($) =>
                                                  $.additional.providerDetailScreen
                                                      .theOAuthCredentialsHaveExpiredMessage,
                                          )
                                        : oauthNeverFreshSentence()
                                }
                                detail={reconnectMessage(
                                    row.is_enabled,
                                    hrefs.providerEdit(row.id),
                                )}
                            />
                        )}
                    </Show>

                    <FactsCard provider={row} />

                    <ModelsCard
                        provider={row}
                        models={models()}
                        modelRoutes={modelRoutes()}
                        pools={pools()}
                        providersById={providersById()}
                        isDiscovering={isDiscovering()}
                        notice={discoveryNotice}
                        onDiscover={() => void discover(row, models())}
                    />

                    <Card>
                        <CardHeader
                            title={translation()(
                                ($) => $.screens.providerDetailScreen.storedAvailability,
                            )}
                            description={translation()(
                                ($) =>
                                    $.screens.providerDetailScreen
                                        .allObservedRestrictionsOnThisAccountResetClearsMessage,
                            )}
                        />
                        <ul class="mt-4">
                            <AccountAvailabilityRow
                                api={props.api}
                                providerId={row.id}
                                provider={row}
                                models={models()}
                                isPoolEnabled={true}
                                upstreamId={null}
                                isAccountInspection={true}
                                canRefreshQuota={false}
                            />
                        </ul>
                    </Card>
                    <ProviderQuotaCard view={quota} />

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

/** What this provider is, read-only: every change lives on the edit page. */
function FactsCard(props: { readonly provider: ProviderResponse }): JSX.Element {
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
            <CardHeader title={translation()(($) => $.screens.providerDetailScreen.details)} />
            <FactList>
                <Fact
                    label={translation()(($) => $.screens.providerDetailScreen.modelPrefix)}
                    value={
                        props.provider.model_prefix === ""
                            ? translation()(
                                  ($) => $.additional.providerDetailScreen.noneExactUpstreamIDs,
                              )
                            : props.provider.model_prefix
                    }
                    isTabular
                />
                <Fact
                    label={translation()(($) => $.screens.providerDetailScreen.type)}
                    value={UPSTREAM_FORMAT.label(props.provider.format)}
                />
                <Fact
                    label={translation()(($) => $.screens.providerDetailScreen.definition)}
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
                {/* The row's stored instance URL, whichever credential kind
                    minted it: the OAuth variant carries the same nullable
                    column the static kind does — an operator types it at the
                    flow's start and a renewal's fold or provisioning
                    extraction may replace it — so the fact renders whenever
                    the row stores one, read-only like every fact here. */}
                <Show when={props.provider.base_url}>
                    {(url) => (
                        <Fact
                            label={translation()(
                                ($) => $.screens.providerDetailScreen.upstreamBaseURL,
                            )}
                            value={url()}
                            isTabular
                            class="break-all"
                        />
                    )}
                </Show>
                <Show when={credentialLine()}>
                    {(line) => (
                        <Fact
                            label={translation()(($) => $.screens.providerDetailScreen.credentials)}
                            value={line()}
                            class="text-ink-dim"
                        />
                    )}
                </Show>
                {/* The one enabled pair this page speaks: the Details card's
                    value echoes the header badge's word exactly — the tight
                    list-row "On/Off" never appears on a detail page. */}
                <Fact
                    label={translation()(($) => $.screens.providerDetailScreen.status)}
                    value={
                        props.provider.is_enabled
                            ? translation()(($) => $.additional.providerDetailScreen.enabled)
                            : translation()(($) => $.additional.providerDetailScreen.disabled)
                    }
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
    readonly modelRoutes: ReadonlyArray<ModelRouteResponse>;
    readonly pools: ReadonlyArray<ProviderPoolResponse>;
    readonly providersById: ReadonlyMap<number, ProviderResponse>;
    readonly isDiscovering: boolean;
    readonly notice: Accessor<ScreenNotice | null>;
    readonly onDiscover: () => void;
}): JSX.Element {
    return (
        <Card>
            <CardHeader
                title={translation()(($) => $.modelCard.heading, {
                    total: props.models.length,
                })}
                description={translation()(
                    ($) =>
                        $.screens.providerDetailScreen.discoveredFromUpstreamOrAddedByHandMessage,
                )}
            >
                <ButtonLink
                    href={hrefs.modelNew(props.provider.id)}
                    glyph="plus"
                >
                    {translation()(($) => $.screens.providerDetailScreen.createModel)}
                </ButtonLink>
                <Button
                    glyph="refresh"
                    isBusy={props.isDiscovering}
                    onClick={props.onDiscover}
                >
                    {props.isDiscovering
                        ? translation()(($) => $.additional.providerDetailScreen.discovering)
                        : translation()(($) => $.additional.providerDetailScreen.discoverModels)}
                </Button>
                {/* The inline list stays unsearchable; the quiet door hands
                    the visitor the searchable models view filtered to this
                    provider — the address the home screen's "Review models"
                    row already speaks. */}
                <ButtonLink
                    href={hrefs.models(props.provider.id)}
                    tone="ghost"
                >
                    {translation()(($) => $.screens.providerDetailScreen.allModels)}
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
                                ? translation()(
                                      ($) =>
                                          $.additional.providerDetailScreen
                                              .askingTheUpstreamWhatItReports,
                                  )
                                : translation()(
                                      ($) =>
                                          $.additional.providerDetailScreen
                                              .noModelsOnThisProviderYetDiscoverWhatMessage,
                                  )}
                        </p>
                    }
                >
                    <ModelList
                        models={props.models}
                        modelRoutes={props.modelRoutes}
                        pools={props.pools}
                        providersById={props.providersById}
                    />
                </Show>
            </div>
        </Card>
    );
}
