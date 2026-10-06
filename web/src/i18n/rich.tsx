import type { JSX } from "@solidjs/web";
import { AppLink } from "/src/ui/app-link";
import { formatNumber } from "./formatters";
import { translation } from "./index";
import type { richMessages } from "./metadata";

/** One rich message's slot-name union, keyed by its declared metadata path. */
type RichSlots = {
    [TEntry in (typeof richMessages)[number] as TEntry["path"]]: TEntry["slots"][number];
};

/** Every metadata-declared rich message path. */
type RichMessagePath = keyof RichSlots & string;

/** Parses authored flat markers only; inserted values are never scanned. */
function splitMarkers(template: string): string[] {
    return template.split(/(\[\[[A-Za-z][A-Za-z0-9]*\]\])/g);
}

/**
 * Renders one declared rich message's template as plain text: every flat
 * [[slot]] marker is replaced by the string passed for it, in the catalog's
 * own order, with no `$`-pattern interpretation of any inserted value. The
 * path is a compile-only witness — it pins `values` to exactly the slots the
 * metadata declares for that message, so a missing or extra slot key fails to
 * compile instead of rendering nothing.
 */
export function renderRichText<TPath extends RichMessagePath>(
    _path: TPath,
    template: string,
    values: Record<RichSlots[TPath], string>,
): string {
    return splitMarkers(template)
        .map((part) =>
            part.startsWith("[[") ? values[part.slice(2, -2) as RichSlots[TPath]] : part,
        )
        .join("");
}

/**
 * Renders one declared rich message's template as JSX, under the same
 * compile-only slot contract as [`renderRichText`]. Every renderer here reads
 * the committed translator through the module-global [`translation`], so the
 * message follows the active catalog wherever the renderer runs.
 */
function renderRichMessage<TPath extends RichMessagePath>(
    _path: TPath,
    template: string,
    values: Record<RichSlots[TPath], JSX.Element>,
): JSX.Element {
    return splitMarkers(template).map((part) =>
        part.startsWith("[[") ? values[part.slice(2, -2) as RichSlots[TPath]] : part,
    );
}

/** The styled in-app link every rich message's link slot renders as. */
function richLink(href: string, label: string | JSX.Element): JSX.Element {
    return (
        <AppLink
            href={href}
            class="font-medium text-accent underline underline-offset-4"
        >
            {label}
        </AppLink>
    );
}
export function connectedMessage(baseUrl: string): JSX.Element {
    return renderRichMessage(
        "rich.connected",
        translation()(($) => $.rich.connected),
        {
            baseUrl: (
                <bdi
                    dir="ltr"
                    class="tnum break-all"
                >
                    {baseUrl}
                </bdi>
            ),
        },
    );
}
export function setupTokenMessage(): JSX.Element {
    return renderRichMessage(
        "rich.setupToken",
        translation()(($) => $.rich.setupToken),
        {
            setting: (
                <bdi
                    dir="ltr"
                    class="tnum"
                >
                    management_api.token
                </bdi>
            ),
        },
    );
}
export function setupCorsMessage(): JSX.Element {
    return renderRichMessage(
        "rich.setupCors",
        translation()(($) => $.rich.setupCors),
        {
            setting: (
                <bdi
                    dir="ltr"
                    class="tnum"
                >
                    management_api.cors.allowed_origin
                </bdi>
            ),
        },
    );
}

export function savedNameMessage(name: string): JSX.Element {
    return renderRichMessage(
        "notices.savedName",
        translation()(($) => $.notices.savedName),
        {
            name: <bdi dir="auto">{name}</bdi>,
        },
    );
}
export function discoveredMessage(providerName: string, count: number): JSX.Element {
    const isolated = <bdi dir="ltr">{formatNumber(count)}</bdi>;
    const provider = <bdi dir="auto">{providerName}</bdi>;
    if (count > 0)
        return renderRichMessage(
            "notices.discovered",
            translation()(($) => $.notices.discovered, {
                count,
            }),
            {
                quantity: isolated,
                providerName: provider,
            },
        );
    return renderRichMessage(
        "notices.nothingDiscovered",
        translation()(($) => $.notices.nothingDiscovered),
        {
            providerName: provider,
        },
    );
}

export function discoveryPendingMessage(providerName: string): JSX.Element {
    return renderRichMessage(
        "operationNotices.discoveryPending",
        translation()(($) => $.operationNotices.discoveryPending),
        {
            providerName: <bdi dir="auto">{providerName}</bdi>,
        },
    );
}

export function keyCountMessage(values: {
    readonly total: number;
    readonly shown: number;
    readonly enabled: number;
    readonly hasSearch: boolean;
}): JSX.Element {
    const parameters = {
        count: values.total,
        total: formatNumber(values.total),
        enabled: formatNumber(values.enabled),
    };
    if (!values.hasSearch) return translation()(($) => $.counts.enabledKeys, parameters);
    return renderRichMessage(
        "counts.filteredKeys",
        translation()(($) => $.counts.filteredKeys, {
            count: values.total,
        }),
        {
            total: <bdi dir="ltr">{parameters.total}</bdi>,
            enabled: <bdi dir="ltr">{parameters.enabled}</bdi>,
            shownSentence: translation()(($) => $.counts.shownKeys, {
                count: values.shown,
                shown: formatNumber(values.shown),
            }),
        },
    );
}
export function requestSummaryMessage(
    total: number,
    peak: {
        readonly requests: number;
        readonly day: string;
    } | null,
): JSX.Element {
    if (peak === null)
        return translation()(($) => $.counts.requests, {
            count: total,
            total: formatNumber(total),
        });
    return renderRichMessage(
        "counts.requestsPeak",
        translation()(($) => $.counts.requestsPeak, {
            count: total,
        }),
        {
            total: <bdi dir="ltr">{formatNumber(total)}</bdi>,
            peak: <bdi dir="ltr">{formatNumber(peak.requests)}</bdi>,
            day: (
                <bdi
                    dir="ltr"
                    class="whitespace-nowrap"
                >
                    {peak.day}
                </bdi>
            ),
        },
    );
}

/** Closed quota messages isolate upstream units and machine dates before insertion. */
export function quotaFactMessage(
    ...args:
        | readonly [
              kind: "counter",
              value: string,
              parameters: {
                  readonly used: string;
                  readonly limit: string;
              },
          ]
        | readonly [
              kind: "remaining",
              value: string,
              parameters: {
                  readonly remaining: string;
              },
          ]
        | readonly [
              kind: "atReset" | "observed",
              value: string,
              parameters: {
                  readonly distance: string;
              },
          ]
        | readonly [
              kind: "dateReset" | "quotaObserved" | "models" | "resets",
              value: string,
          ]
): JSX.Element {
    const value = args[1];
    const isolated = <bdi dir="ltr">{value}</bdi>;
    switch (args[0]) {
        case "counter":
            return renderRichMessage(
                "quotaMessages.counter",
                translation()(($) => $.quotaMessages.counter),
                {
                    used: <bdi dir="ltr">{args[2].used}</bdi>,
                    limit: <bdi dir="ltr">{args[2].limit}</bdi>,
                    unit: isolated,
                },
            );
        case "remaining":
            return renderRichMessage(
                "quotaMessages.remaining",
                translation()(($) => $.quotaMessages.remaining),
                {
                    remaining: <bdi dir="ltr">{args[2].remaining}</bdi>,
                    unit: isolated,
                },
            );
        case "atReset":
            return renderRichMessage(
                "quotaMessages.atReset",
                translation()(($) => $.quotaMessages.atReset),
                {
                    distance: args[2].distance,
                    instant: isolated,
                },
            );
        case "observed":
            return renderRichMessage(
                "quotaMessages.observed",
                translation()(($) => $.quotaMessages.observed),
                {
                    distance: args[2].distance,
                    instant: isolated,
                },
            );
        case "dateReset":
            return renderRichMessage(
                "quotaMessages.dateReset",
                translation()(($) => $.quotaMessages.dateReset),
                {
                    date: isolated,
                },
            );
        case "quotaObserved":
            return renderRichMessage(
                "quotaMessages.quotaObserved",
                translation()(($) => $.quotaMessages.quotaObserved),
                {
                    instant: isolated,
                },
            );
        case "models":
            return renderRichMessage(
                "quotaMessages.models",
                translation()(($) => $.quotaMessages.models),
                {
                    identifiers: isolated,
                },
            );
        case "resets":
            return renderRichMessage(
                "quotaMessages.resets",
                translation()(($) => $.quotaMessages.resets),
                {
                    instant: isolated,
                },
            );
    }
}

export function editNameMessage(name: string): JSX.Element {
    return renderRichMessage(
        "entityMessages.editName",
        translation()(($) => $.entityMessages.editName),
        {
            name: <bdi dir="auto">{name}</bdi>,
        },
    );
}

export function editIdentifierMessage(identifier: string): JSX.Element {
    return renderRichMessage(
        "entityMessages.editIdentifier",
        translation()(($) => $.entityMessages.editIdentifier),
        {
            identifier: <bdi dir="ltr">{identifier}</bdi>,
        },
    );
}

export function deleteKeyMessage(name: string): JSX.Element {
    return renderRichMessage(
        "entityMessages.deleteKey",
        translation()(($) => $.entityMessages.deleteKey),
        {
            name: <bdi dir="auto">{name}</bdi>,
        },
    );
}

export function deleteLastKeyMessage(name: string): JSX.Element {
    return renderRichMessage(
        "entityMessages.deleteLastKey",
        translation()(($) => $.entityMessages.deleteLastKey),
        {
            name: <bdi dir="auto">{name}</bdi>,
        },
    );
}

export function deleteModelMessage(identifier: string): JSX.Element {
    return renderRichMessage(
        "entityMessages.deleteModel",
        translation()(($) => $.entityMessages.deleteModel),
        {
            identifier: <bdi dir="ltr">{identifier}</bdi>,
        },
    );
}

export function deleteProviderMessage(name: string): JSX.Element {
    return renderRichMessage(
        "entityMessages.deleteProvider",
        translation()(($) => $.entityMessages.deleteProvider),
        {
            name: <bdi dir="auto">{name}</bdi>,
        },
    );
}

export function switchPresetMessage(name: string): JSX.Element {
    return renderRichMessage(
        "entityMessages.switchPreset",
        translation()(($) => $.entityMessages.switchPreset),
        {
            name: <bdi dir="auto">{name}</bdi>,
        },
    );
}

export function noKeysMatchMessage(query: string): JSX.Element {
    return renderRichMessage(
        "entityMessages.noKeysMatch",
        translation()(($) => $.entityMessages.noKeysMatch),
        {
            query: <bdi dir="auto">{query}</bdi>,
        },
    );
}

export function noProvidersMatchMessage(query: string): JSX.Element {
    return renderRichMessage(
        "entityMessages.noProvidersMatch",
        translation()(($) => $.entityMessages.noProvidersMatch),
        {
            query: <bdi dir="auto">{query}</bdi>,
        },
    );
}

export function noModelsMatchMessage(query: string): JSX.Element {
    return renderRichMessage(
        "entityMessages.noModelsMatch",
        translation()(($) => $.entityMessages.noModelsMatch),
        {
            query: <bdi dir="auto">{query}</bdi>,
        },
    );
}

export function noProviderModelsMatchMessage(query: string): JSX.Element {
    return renderRichMessage(
        "entityMessages.noProviderModelsMatch",
        translation()(($) => $.entityMessages.noProviderModelsMatch),
        {
            query: <bdi dir="auto">{query}</bdi>,
        },
    );
}

export function missingPathMessage(path: string): JSX.Element {
    return renderRichMessage(
        "entityMessages.missingPath",
        translation()(($) => $.entityMessages.missingPath),
        {
            path: <bdi dir="ltr">{path}</bdi>,
        },
    );
}

export function aliasCreatedMessage(identifier: string): JSX.Element {
    return renderRichMessage(
        "entityMessages.aliasCreated",
        translation()(($) => $.entityMessages.aliasCreated),
        {
            identifier: <bdi dir="ltr">{identifier}</bdi>,
        },
    );
}

export function addAliasMessage(identifier: string): JSX.Element {
    return renderRichMessage(
        "entityMessages.addAlias",
        translation()(($) => $.entityMessages.addAlias),
        {
            identifier: <bdi dir="ltr">{identifier}</bdi>,
        },
    );
}

export function poolMembersMessage(count: number, prefix: string): JSX.Element {
    return renderRichMessage(
        "routingMessages.poolMembers",
        translation()(($) => $.routingMessages.poolMembers, {
            count,
        }),
        {
            quantity: <bdi dir="ltr">{formatNumber(count)}</bdi>,
            prefix: <bdi dir="ltr">{prefix}</bdi>,
        },
    );
}
export function directTargetMessage(provider: string, model: string): JSX.Element {
    return renderRichMessage(
        "routingMessages.direct",
        translation()(($) => $.routingMessages.direct),
        {
            provider: <bdi dir="auto">{provider}</bdi>,
            model: <bdi dir="ltr">{model}</bdi>,
        },
    );
}
export function missingDirectTargetMessage(id: number): JSX.Element {
    return renderRichMessage(
        "routingMessages.directMissing",
        translation()(($) => $.routingMessages.directMissing),
        {
            id: <bdi dir="ltr">{formatNumber(id)}</bdi>,
        },
    );
}
export function poolTargetMessage(pool: string, model: string): JSX.Element {
    return renderRichMessage(
        "routingMessages.pool",
        translation()(($) => $.routingMessages.pool),
        {
            pool: <bdi dir="auto">{pool}</bdi>,
            model: <bdi dir="ltr">{model}</bdi>,
        },
    );
}
export function publicIdsMessage(identifiers: string): JSX.Element {
    return renderRichMessage(
        "routingMessages.publicIds",
        translation()(($) => $.routingMessages.publicIds),
        {
            identifiers: <bdi dir="ltr">{identifiers}</bdi>,
        },
    );
}
export function reconnectMessage(isEnabled: boolean, href: string): JSX.Element {
    const editLink = richLink(
        href,
        translation()(($) => $.screens.providerDetailScreen.editPage),
    );
    return isEnabled
        ? renderRichMessage(
              "routingMessages.reconnect",
              translation()(($) => $.routingMessages.reconnect),
              {
                  editLink,
              },
          )
        : renderRichMessage(
              "routingMessages.reconnectDisabled",
              translation()(($) => $.routingMessages.reconnectDisabled),
              {
                  editLink,
              },
          );
}

export function networkTitleMessage(
    kind: "unreachable" | "mixed-content" | "blocked" | "no-answer",
    baseUrl: string,
): JSX.Element {
    const template =
        kind === "unreachable"
            ? translation()(($) => $.diagnosis.nothingAnsweredAtBaseUrl)
            : kind === "mixed-content"
              ? translation()(($) => $.diagnosis.anHTTPSPageCannotCallTheHTTPAddressBaseUrl)
              : kind === "blocked"
                ? translation()(
                      ($) =>
                          $.diagnosis.theGatewayAnsweredButTheBrowserBlockedTheResponseFromBaseUrl,
                  )
                : translation()(($) => $.diagnosis.noAnswerFromBaseUrl);
    return renderRichMessage(
        kind === "unreachable"
            ? "diagnosis.nothingAnsweredAtBaseUrl"
            : kind === "mixed-content"
              ? "diagnosis.anHTTPSPageCannotCallTheHTTPAddressBaseUrl"
              : kind === "blocked"
                ? "diagnosis.theGatewayAnsweredButTheBrowserBlockedTheResponseFromBaseUrl"
                : "diagnosis.noAnswerFromBaseUrl",
        template,
        {
            baseUrl: <bdi dir="ltr">{baseUrl}</bdi>,
        },
    );
}
export function corsRecoveryMessage(isConfirmed: boolean, origin: string): JSX.Element {
    return isConfirmed
        ? renderRichMessage(
              "diagnosis.addOriginToSettingInThatInstancesConfigurationFileAndRestartTheGatewayServing",
              translation()(
                  ($) =>
                      $.diagnosis
                          .addOriginToSettingInThatInstancesConfigurationFileAndRestartTheGatewayServing,
              ),
              {
                  origin: <bdi dir="ltr">{origin}</bdi>,
                  setting: <bdi dir="ltr">management_api.cors.allowed_origin</bdi>,
              },
          )
        : renderRichMessage(
              "diagnosis.eitherNothingIsListeningAtThatAddressOrTheBrowserBlockedTheResponseBefore",
              translation()(
                  ($) =>
                      $.diagnosis
                          .eitherNothingIsListeningAtThatAddressOrTheBrowserBlockedTheResponseBefore,
              ),
              {
                  origin: <bdi dir="ltr">{origin}</bdi>,
                  setting: <bdi dir="ltr">management_api.cors.allowed_origin</bdi>,
              },
          );
}

export function ownerDefaultMessage(name: string, kind: "provider" | "pool"): JSX.Element {
    const nameSlot = <bdi dir="auto">{name}</bdi>;
    return kind === "provider"
        ? renderRichMessage(
              "routingMessages.ownerDefault.provider",
              translation()(($) => $.routingMessages.ownerDefault.provider),
              {
                  name: nameSlot,
              },
          )
        : renderRichMessage(
              "routingMessages.ownerDefault.pool",
              translation()(($) => $.routingMessages.ownerDefault.pool),
              {
                  name: nameSlot,
              },
          );
}
export function modelAccessMessage(identifier: string): JSX.Element {
    return renderRichMessage(
        "routingMessages.modelAccess",
        translation()(($) => $.routingMessages.modelAccess),
        {
            identifier: <bdi dir="ltr">{identifier}</bdi>,
        },
    );
}
export function directAccessTargetMessage(id: number): JSX.Element {
    return renderRichMessage(
        "routingMessages.directTarget",
        translation()(($) => $.routingMessages.directTarget),
        {
            id: <bdi dir="ltr">{formatNumber(id)}</bdi>,
        },
    );
}
export function poolAccessTargetMessage(identifier: string): JSX.Element {
    return renderRichMessage(
        "routingMessages.poolTarget",
        translation()(($) => $.routingMessages.poolTarget),
        {
            identifier: <bdi dir="ltr">{identifier}</bdi>,
        },
    );
}

export function keyEndpointMessage(overviewHref: string): JSX.Element {
    return renderRichMessage(
        "linkedMessages.keyEndpoint",
        translation()(($) => $.linkedMessages.keyEndpoint),
        {
            overviewLink: richLink(
                overviewHref,
                translation()(($) => $.screens.apiKeyDetailScreen.overview),
            ),
        },
    );
}
export function servingKeyMessage(keysHref: string): JSX.Element {
    return renderRichMessage(
        "linkedMessages.servingKey",
        translation()(($) => $.linkedMessages.servingKey),
        {
            keysLink: richLink(
                keysHref,
                translation()(($) => $.screens.homeScreen.apiKeys),
            ),
        },
    );
}
export function vanishedModelMessage(providerHref: string, claimHref: string): JSX.Element {
    return renderRichMessage(
        "linkedMessages.vanished",
        translation()(($) => $.linkedMessages.vanished),
        {
            discoverLink: richLink(
                providerHref,
                translation()(($) => $.linkedMessages.discoverLabel),
            ),
            claimLink: richLink(
                claimHref,
                translation()(($) => $.linkedMessages.claimLabel),
            ),
        },
    );
}
export function inventoryAttentionMessage(
    name: string,
    reason: "no-models" | "no-ready-models",
): JSX.Element {
    const nameSlot = (
        <bdi
            dir="auto"
            class="font-medium text-ink"
        >
            {name}
        </bdi>
    );
    return reason === "no-models"
        ? renderRichMessage(
              "linkedMessages.inventoryEmpty",
              translation()(($) => $.linkedMessages.inventoryEmpty),
              {
                  name: nameSlot,
              },
          )
        : renderRichMessage(
              "linkedMessages.inventoryUnready",
              translation()(($) => $.linkedMessages.inventoryUnready),
              {
                  name: nameSlot,
              },
          );
}
export function requestChartPointMessage(
    day: string,
    count: number,
    quantity: string,
): {
    readonly accessibleName: string;
    readonly content: JSX.Element;
} {
    const template = translation()(($) => $.linkedMessages.chartPoint, {
        count,
    });
    return {
        accessibleName: renderRichText("linkedMessages.chartPoint", template, {
            day,
            quantity,
        }),
        content: renderRichMessage("linkedMessages.chartPoint", template, {
            day: (
                <bdi
                    dir="ltr"
                    class="whitespace-nowrap"
                >
                    {day}
                </bdi>
            ),
            quantity: <bdi dir="ltr">{quantity}</bdi>,
        }),
    };
}
