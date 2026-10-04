import type { JSX } from "@solidjs/web";
import { For } from "solid-js";
import { translation } from "/src/i18n";
import { formatNumber } from "/src/i18n/formatters";
import { publicIdsMessage } from "/src/i18n/rich";
import type {
    ModelResponse,
    ModelRouteResponse,
    ProviderPoolResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import {
    automaticModelNames,
    effectiveCompactionFacts,
    isModelInventoryReady,
} from "/src/management-api/serving";
import {
    COMPACTION_MECHANISM,
    type CompactionMechanism,
    DISCOVERY_STATE,
} from "/src/management-api/vocabulary";
import { hrefs } from "/src/router";
import { ButtonLink } from "/src/ui/controls";
import { Badge, type BadgeMark } from "/src/ui/feedback";
import { RowLink } from "/src/ui/row-link";

/**
 * How one model reads at a glance, under the gateway's three-input effective
 * inventory availability derived client-side from rows the app already fetches
 * and never read off the wire: `Ready`, or the one cause that keeps it from
 * serving — the model's own switch (`Off`), the upstream no longer reporting
 * the row (`Suspended`), the provider's switch (`Held by provider`), or a
 * provider row absent from the fetched rows (`Unknown`).
 */
export function modelInventoryBadge(
    model: ModelResponse,
    provider: ProviderResponse | undefined,
): BadgeMark {
    if (isModelInventoryReady(model, provider)) {
        return {
            tone: "ok",
            get label() {
                return translation()(($) => $.helpers.modelsTable.ready);
            },
        };
    }

    if (!model.is_enabled) {
        return {
            tone: "muted",
            get label() {
                return translation()(($) => $.helpers.modelsTable.off);
            },
        };
    }

    if (model.discovery_state === "vanished") {
        return {
            tone: "warn",
            get label() {
                return translation()(($) => $.helpers.modelsTable.suspended);
            },
        };
    }

    if (provider !== undefined && !provider.is_enabled) {
        return {
            tone: "warn",
            get label() {
                return translation()(($) => $.helpers.modelsTable.heldByProvider);
            },
        };
    }

    // No switch is off and the row is listed, yet the provider row the
    // conjunction needs is missing from the fetched rows — reachable only
    // while a concurrent provider deletion races the listing read; the next
    // read settles the verdict.
    return {
        tone: "muted",
        get label() {
            return translation()(($) => $.helpers.modelsTable.unknown);
        },
    };
}

/**
 * Where one row stands in its discovery lifecycle: discovered from upstream,
 * added by hand, or no longer listed. A row the upstream no longer reports
 * keeps its curated settings and can be claimed back; it never serves
 * meanwhile.
 */
export function modelDiscoveryBadge(model: ModelResponse): BadgeMark {
    switch (model.discovery_state) {
        case "discovered":
            return {
                tone: "muted",
                label: DISCOVERY_STATE.label("discovered"),
            };

        case "manual":
            return {
                tone: "info",
                label: DISCOVERY_STATE.label("manual"),
            };

        case "vanished":
            return {
                tone: "warn",
                label: DISCOVERY_STATE.label("vanished"),
            };
    }
}

/**
 * The badges' short mechanism names, one per compaction mechanism — the
 * tight row spelling beside the vocabulary's long form the model page uses.
 * The record is exhaustive, so a mechanism added to the vocabulary fails to
 * compile until a short name stands here.
 */
const COMPACTION_BADGE_NAMES: Record<CompactionMechanism, string> = {
    responses_standalone_compact: "compact",
    responses_explicit_trigger: "trigger",
    responses_automatic_compaction: "auto",
    messages_on_demand_compaction: "on-demand",
    messages_threshold_compaction: "threshold",
};

/**
 * One row's compaction badges, one per mechanism the row itself states a
 * fact for — the administrator's override or the latest discovery knowledge
 * (the row's two resolution layers, derived client-side under rule 7). A
 * mechanism both layers leave unstated is omitted: its verdict is the
 * definition's declaration, which the row never carries, so naming it here
 * would claim a fact nobody stated. A stated supported fact reads the
 * mechanism's name; a stated unsupported fact reads "no" over it.
 */
export function modelCompactionBadges(model: ModelResponse): ReadonlyArray<BadgeMark> {
    const facts = effectiveCompactionFacts(model);

    return COMPACTION_MECHANISM.options.flatMap(({ value }) => {
        const { support } = facts[value];

        if (support === "unknown") {
            return [];
        }

        return [
            {
                tone: support === "supported" ? "ok" : "muted",
                label:
                    support === "supported"
                        ? COMPACTION_BADGE_NAMES[value]
                        : `no ${COMPACTION_BADGE_NAMES[value]}`,
            },
        ];
    });
}

/**
 * One physical model's upstream identity, inventory availability, and discovery state. The row is one link to the model's own
 * page, where the settings behind those facts are changed.
 */
function ModelRow(props: {
    readonly model: ModelResponse;
    readonly directRouteCount: number;
    readonly automaticNames: ReadonlyArray<string>;
    readonly provider: ProviderResponse | undefined;
    readonly showProvider?: boolean | undefined;
}): JSX.Element {
    const serving = (): BadgeMark => modelInventoryBadge(props.model, props.provider);

    const discovery = (): BadgeMark => modelDiscoveryBadge(props.model);

    const compaction = (): ReadonlyArray<BadgeMark> => modelCompactionBadges(props.model);

    const subline = (): JSX.Element => (
        <>
            {props.provider === undefined ? (
                ""
            ) : (
                <>
                    {publicIdsMessage(
                        props.automaticNames.length === 0
                            ? translation()(($) => $.helpers.modelsTable.noAutomaticEntry)
                            : props.automaticNames.join(", "),
                    )}{" "}
                    ·{" "}
                </>
            )}
            {props.showProvider === true ? (
                <>
                    <bdi dir="auto">{props.model.provider}</bdi> ·{" "}
                </>
            ) : (
                ""
            )}
            {props.directRouteCount === 0
                ? translation()(($) => $.helpers.modelsTable.noDirectRoutes)
                : translation()(($) => $.routingMessages.directRoutes, {
                      count: props.directRouteCount,
                      quantity: formatNumber(props.directRouteCount),
                  })}
        </>
    );

    return (
        <RowLink
            href={hrefs.modelDetail(props.model.id)}
            primary={
                <span class="tnum block truncate text-[13.5px] font-medium">
                    {props.model.upstream_id}
                </span>
            }
            secondary={subline()}
            trailing={
                <ButtonLink href={hrefs.modelRouteNew(props.model.id)}>
                    {translation()(($) => $.controls.addAlias)}
                </ButtonLink>
            }
            badges={
                <>
                    <Badge
                        tone={serving().tone}
                        label={serving().label}
                    />
                    <Badge
                        tone={discovery().tone}
                        label={discovery().label}
                    />
                    <For each={compaction()}>
                        {(badge) => (
                            <Badge
                                tone={badge.tone}
                                label={badge.label}
                            />
                        )}
                    </For>
                </>
            }
        />
    );
}

/** A list of model rows, newest reading first as the gateway orders them. */
export function ModelList(props: {
    readonly models: ReadonlyArray<ModelResponse>;
    readonly modelRoutes: ReadonlyArray<ModelRouteResponse>;
    readonly pools: ReadonlyArray<ProviderPoolResponse>;
    readonly providersById: ReadonlyMap<number, ProviderResponse>;
    readonly showProvider?: boolean | undefined;
}): JSX.Element {
    return (
        <ul class="flex flex-col gap-2">
            <For each={props.models}>
                {(model) => (
                    <ModelRow
                        model={model}
                        automaticNames={automaticModelNames(model, props.modelRoutes, props.pools)}
                        directRouteCount={
                            props.modelRoutes.filter(
                                (route) =>
                                    route.target.type === "direct" &&
                                    route.target.model_id === model.id,
                            ).length
                        }
                        provider={props.providersById.get(model.provider_id)}
                        showProvider={props.showProvider}
                    />
                )}
            </For>
        </ul>
    );
}
