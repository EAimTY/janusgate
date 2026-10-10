import type { JSX } from "@solidjs/web";
import { createSignal, For, Show } from "solid-js";
import { readPublishedInventory } from "/src/features/inventory/read";
import { isSameModelRow, isSameProviderRow } from "/src/features/inventory/row-equality";
import { modelDiscoveryBadge, modelInventoryBadge } from "/src/features/models/model-list";
import { VanishedModelNotice } from "/src/features/models/vanished-model-notice";
import { EntityUsageCard } from "/src/features/usage/stats";
import { formatTokenCount } from "/src/format";
import { translation } from "/src/i18n";
import type { ManagementApi } from "/src/management-api/client";
import type {
    ModelResponse,
    ModelRouteResponse,
    ProviderPoolResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import {
    automaticModelNames,
    type CapabilitySource,
    type CompactionFact,
    capabilitySource,
    effectiveCompactionFacts,
    effectiveMaxInputTokens,
    effectiveMaxOutputTokens,
    effectiveThinkingLevels,
} from "/src/management-api/serving";
import {
    COMPACTION_MECHANISM,
    COMPACTION_MECHANISMS_BY_FORMAT,
    type CompactionMechanism,
    DISCOVERY_STATE,
    MECHANISM_SUPPORT,
} from "/src/management-api/vocabulary";
import { hrefs } from "/src/router";
import { createScreenRead, readOne } from "/src/screen-state/read";
import { createPinnedRow } from "/src/screen-state/row-state";
import { AppLink } from "/src/ui/app-link";
import { proseLinkClass } from "/src/ui/attributes";
import { ButtonLink } from "/src/ui/controls";
import { Badge, type BadgeMark } from "/src/ui/feedback";
import { Card, CardHeader, Fact, FactList, PageHeader } from "/src/ui/layout";
import { RowNotFound } from "/src/ui/row-not-found";
import { ScreenFrame } from "/src/ui/screen-frame";

/**
 * One capability fact's provenance line: the words every effective
 * declaration states beside its value, naming the layer the fact came
 * from — the administrator's override, the latest discovery knowledge, or
 * unknown. The unknown wording is each fact family's own: a row-column
 * declaration unknown here was omitted from the listings, while a compaction
 * mechanism without a row fact defers to the definition's declaration, so
 * `unknownWording` rewords that one case.
 */
function provenanceLabel(source: CapabilitySource, unknownWording?: string): string {
    switch (source) {
        case "override":
            return translation()(
                ($) => $.screens.modelDetailScreen.overriddenByAdministratorMessage,
            );
        case "discovered":
            return translation()(($) => $.screens.modelDetailScreen.fromTheLatestDiscoveryMessage);
        case "unknown":
            return (
                unknownWording ??
                translation()(
                    ($) => $.screens.modelDetailScreen.unknownOmittedFromModelListingsMessage,
                )
            );
    }
}

/**
 * One model's page: what it is, where it stands, and what serving clients
 * did with it. Nothing is changed here — the edit page owns every change.
 */
export function ModelDetailScreen(props: {
    readonly api: ManagementApi;
    readonly modelId: number;
}): JSX.Element {
    const [providers, setProviders] = createSignal<ReadonlyArray<ProviderResponse>>([]);
    const [models, setModels] = createSignal<ReadonlyArray<ModelResponse>>([]);
    const [modelRoutes, setModelRoutes] = createSignal<ReadonlyArray<ModelRouteResponse>>([]);
    const [pools, setPools] = createSignal<ReadonlyArray<ProviderPoolResponse>>([]);

    const read = createScreenRead({
        load: async (signal) => {
            const catalog = await readOne(signal, readPublishedInventory(props.api, null));

            setProviders(catalog.providers);
            setModels(catalog.models);
            setModelRoutes(catalog.modelRoutes);
            setPools(catalog.pools);
        },
    });

    // Each row keeps its previous answer when a re-read returns the same
    // content (`createPinnedRow`), so a refresh never flips object identity
    // and never remounts the cards (or restarts the usage read) over
    // unchanged data. The owning provider is a lookup beside the model, not
    // the screen's subject, so it pins without holding.
    const model = createPinnedRow(
        () => models().find((row) => row.id === props.modelId) ?? null,
        isSameModelRow,
    );

    const provider = createPinnedRow(() => {
        const current = model();
        return current === null
            ? null
            : (providers().find((row) => row.id === current.provider_id) ?? null);
    }, isSameProviderRow);

    return (
        <ScreenFrame
            read={read}
            row={() => {
                const current = model();
                return current === null
                    ? null
                    : {
                          model: current,
                          provider: provider(),
                      };
            }}
            firstReadHeader={
                <PageHeader
                    title={translation()(($) => $.screens.modelDetailScreen.model)}
                    backHref={hrefs.models(null)}
                    backLabel={translation()(($) => $.screens.modelDetailScreen.models)}
                />
            }
            notFound={
                <RowNotFound
                    kind="model"
                    headerTitle={translation()(($) => $.screens.modelDetailScreen.model)}
                />
            }
        >
            {(both) => (
                <ModelFacts
                    api={props.api}
                    provider={both.provider}
                    model={both.model}
                    modelRoutes={modelRoutes()}
                    pools={pools()}
                />
            )}
        </ScreenFrame>
    );
}

/**
 * One model's capability declarations (`docs/design/storage.md`): each effective
 * value (`override ?? base`, derived client-side under rule 7) with the
 * provenance the listings render from — the administrator's override, the
 * latest discovery knowledge, or unknown, which every listing omits. A
 * declaration, never an enforcement: the upstream stays the sole authority.
 * The compaction mechanisms the owning provider's format can speak state
 * their effective facts (`override ?? base`, the row's two layers) the same
 * way — the definition's declaration, the resolution's third layer, is
 * definition data the model row never carries.
 */
function CapabilityFacts(props: {
    readonly model: ModelResponse;
    readonly provider: ProviderResponse | null;
}): JSX.Element {
    const levels = (): ReadonlyArray<string> | null => effectiveThinkingLevels(props.model);
    const maxInput = (): number | null => effectiveMaxInputTokens(props.model);
    const maxOutput = (): number | null => effectiveMaxOutputTokens(props.model);
    const compaction = (): Readonly<Record<CompactionMechanism, CompactionFact>> =>
        effectiveCompactionFacts(props.model);

    const levelsSource = (): string =>
        provenanceLabel(
            capabilitySource(props.model.thinking_levels_override, props.model.thinking_levels),
        );

    const tokenSource = (override: number | null, base: number | null): string =>
        provenanceLabel(capabilitySource(override, base));

    const compactionSource = (fact: CompactionFact): string =>
        provenanceLabel(
            fact.source,
            translation()(
                ($) => $.screens.modelDetailScreen.noFactOnThisRowTheDefinitionsDeclarationMessage,
            ),
        );

    const levelsDisplay = (): string => {
        const effective = levels();

        if (effective === null) {
            return translation()(($) => $.screens.modelDetailScreen.unknown);
        }

        return effective.length === 0
            ? translation()(($) => $.screens.modelDetailScreen.noLevels)
            : effective.join(", ");
    };

    // The mechanisms the owning provider's format can speak, from the
    // vocabulary's per-format table beside the mechanism vocabulary itself.
    // Without the provider row at hand the section stays unstated rather than
    // guessing the format.
    const compactionMechanisms = (): ReadonlyArray<CompactionMechanism> =>
        props.provider === null ? [] : COMPACTION_MECHANISMS_BY_FORMAT[props.provider.format];

    return (
        <Card>
            <CardHeader
                title={translation()(($) => $.screens.modelDetailScreen.capabilities)}
                description={translation()(
                    ($) =>
                        $.screens.modelDetailScreen.whatTheModelListingsDeclareForThisModelMessage,
                )}
            />
            <FactList>
                <Fact
                    label={translation()(($) => $.screens.modelDetailScreen.reasoningLevels)}
                    value={
                        <span>
                            <span class="tnum">{levelsDisplay()}</span>
                            <span class="block text-[12.5px] text-ink-faint">{levelsSource()}</span>
                        </span>
                    }
                    class="break-all"
                />
                <Fact
                    label={translation()(($) => $.screens.modelDetailScreen.maxInputTokens)}
                    value={
                        <span>
                            <span class="tnum">{formatTokenCount(maxInput())}</span>
                            <span class="block text-[12.5px] text-ink-faint">
                                {tokenSource(
                                    props.model.max_input_tokens_override,
                                    props.model.max_input_tokens,
                                )}
                            </span>
                        </span>
                    }
                />
                <Fact
                    label={translation()(($) => $.screens.modelDetailScreen.maxOutputTokens)}
                    value={
                        <span>
                            <span class="tnum">{formatTokenCount(maxOutput())}</span>
                            <span class="block text-[12.5px] text-ink-faint">
                                {tokenSource(
                                    props.model.max_output_tokens_override,
                                    props.model.max_output_tokens,
                                )}
                            </span>
                        </span>
                    }
                />
            </FactList>
            <Show when={compactionMechanisms().length > 0}>
                <div class="mt-5">
                    <h3 class="text-[13px] font-medium text-ink">
                        {translation()(($) => $.screens.modelDetailScreen.compaction)}
                    </h3>
                    <p class="mt-1 max-w-[70ch] text-[13px] text-ink-faint">
                        {translation()(
                            ($) =>
                                $.screens.modelDetailScreen
                                    .theRowsOwnCompactionFactsTheOverrideElseMessage,
                        )}
                    </p>
                    <FactList>
                        <For each={compactionMechanisms()}>
                            {(mechanism) => {
                                const fact = compaction()[mechanism];

                                return (
                                    <Fact
                                        label={COMPACTION_MECHANISM.label(mechanism)}
                                        value={
                                            <span>
                                                <span class="tnum">
                                                    {MECHANISM_SUPPORT.label(fact.support)}
                                                </span>
                                                <span class="block text-[12.5px] text-ink-faint">
                                                    {compactionSource(fact)}
                                                </span>
                                            </span>
                                        }
                                    />
                                );
                            }}
                        </For>
                    </FactList>
                </div>
            </Show>
        </Card>
    );
}

function ModelFacts(props: {
    readonly api: ManagementApi;
    readonly provider: ProviderResponse | null;
    readonly model: ModelResponse;
    readonly modelRoutes: ReadonlyArray<ModelRouteResponse>;
    readonly pools: ReadonlyArray<ProviderPoolResponse>;
}): JSX.Element {
    const serving = (): BadgeMark => modelInventoryBadge(props.model, props.provider ?? undefined);

    const discovery = (): BadgeMark => modelDiscoveryBadge(props.model);

    return (
        <>
            <PageHeader
                title={props.model.upstream_id}
                description={translation()(($) => $.screens.modelDetailScreen.upstreamOnProvider, {
                    upstreamId: props.model.upstream_id,
                    provider: props.model.provider,
                })}
                backHref={hrefs.models(props.model.provider_id)}
                backLabel={translation()(($) => $.screens.modelDetailScreen.modelsBackLabel)}
            >
                <Badge
                    tone={serving().tone}
                    label={serving().label}
                />
                <Badge
                    tone={discovery().tone}
                    label={discovery().label}
                />
                <ButtonLink href={hrefs.modelRouteNew(props.model.id)}>
                    {translation()(($) => $.screens.modelDetailScreen.addAlias)}
                </ButtonLink>
                <ButtonLink href={hrefs.modelEdit(props.model.id)}>
                    {translation()(($) => $.screens.modelDetailScreen.editModel)}
                </ButtonLink>
            </PageHeader>

            <VanishedModelNotice
                isVanished={props.model.discovery_state === "vanished"}
                providerId={props.model.provider_id}
                upstreamId={props.model.upstream_id}
            />

            <Card>
                <CardHeader title={translation()(($) => $.screens.modelDetailScreen.details)} />
                <FactList>
                    <Fact
                        label={translation()(($) => $.screens.modelDetailScreen.publicModelNames)}
                        value={
                            automaticModelNames(props.model, props.modelRoutes, props.pools).join(
                                ", ",
                            ) || translation()(($) => $.screens.modelDetailScreen.noAutomaticEntry)
                        }
                        isTabular
                        class="break-all"
                    />
                    <Fact
                        label={translation()(($) => $.screens.modelDetailScreen.directRoutes)}
                        value={String(
                            props.modelRoutes.filter(
                                (route) =>
                                    route.target.type === "direct" &&
                                    route.target.model_id === props.model.id,
                            ).length,
                        )}
                        isTabular
                    />
                    <Fact
                        label={translation()(($) => $.screens.modelDetailScreen.upstreamID)}
                        value={props.model.upstream_id}
                        isTabular
                        class="break-all"
                    />
                    <Fact
                        label={translation()(($) => $.screens.modelDetailScreen.provider)}
                        value={
                            props.provider === null ? (
                                <span class="text-ink-dim">{props.model.provider}</span>
                            ) : (
                                <AppLink
                                    href={hrefs.providerDetail(props.provider.id)}
                                    class={proseLinkClass}
                                >
                                    {props.provider.name}
                                </AppLink>
                            )
                        }
                    />
                    <Fact
                        label={translation()(($) => $.screens.modelDetailScreen.discovery)}
                        value={DISCOVERY_STATE.label(props.model.discovery_state)}
                        class="text-ink-dim"
                    />
                    {/* The one enabled pair this page speaks: the Details
                        card's Status echoes the enabled vocabulary every
                        detail page shares — the tight list-row "On/Off" never
                        appears here, while the header badge stays the
                        effective-serving ladder above. */}
                    <Fact
                        label={translation()(($) => $.screens.modelDetailScreen.status)}
                        value={
                            props.model.is_enabled
                                ? translation()(($) => $.screens.modelDetailScreen.enabled)
                                : translation()(($) => $.screens.modelDetailScreen.disabled)
                        }
                    />
                </FactList>
            </Card>

            <CapabilityFacts
                model={props.model}
                provider={props.provider}
            />

            <EntityUsageCard
                api={props.api}
                filter={{
                    model_id: props.model.id,
                }}
                subjectKind="model"
            />
        </>
    );
}
