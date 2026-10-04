import type { JSX } from "@solidjs/web";
import type { TFunction } from "i18next";
import { AppLink } from "/src/ui/app-link";
import { formatNumber } from "./formatters";
import { translation } from "./index";

/** Parses authored flat markers only; inserted elements are never scanned. */
function slots(template: string, values: Readonly<Record<string, JSX.Element>>): JSX.Element {
    return template.split(/(\[\[[A-Za-z][A-Za-z0-9]*\]\])/g).map((part) => {
        if (!part.startsWith("[[")) return part;
        return values[part.slice(2, -2)];
    });
}
export function connectedMessage(t: TFunction<"translation">, baseUrl: string): JSX.Element {
    return slots(
        t(($) => $.rich.connected),
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
export function providerModelMessage(
    t: TFunction<"translation">,
    values: {
        readonly providerName: string;
        readonly providerHref: string;
        readonly modelId: string;
    },
): JSX.Element {
    return slots(
        t(($) => $.rich.providerModel),
        {
            providerLink: (
                <AppLink href={values.providerHref}>
                    <bdi dir="auto">{values.providerName}</bdi>
                </AppLink>
            ),
            modelId: (
                <bdi
                    dir="ltr"
                    class="tnum"
                >
                    {values.modelId}
                </bdi>
            ),
        },
    );
}
export function setupTokenMessage(t: TFunction<"translation">): JSX.Element {
    return slots(
        t(($) => $.rich.setupToken),
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
export function setupCorsMessage(t: TFunction<"translation">): JSX.Element {
    return slots(
        t(($) => $.rich.setupCors),
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

export function savedNameMessage(t: TFunction<"translation">, name: string): JSX.Element {
    return slots(
        t(($) => $.notices.savedName),
        {
            name: <bdi dir="auto">{name}</bdi>,
        },
    );
}
export function discoveredMessage(
    t: TFunction<"translation">,
    providerName: string,
    count: number,
): JSX.Element {
    const template =
        count > 0
            ? t(($) => $.notices.discovered, {
                  count,
              })
            : t(($) => $.notices.nothingDiscovered);
    return slots(template, {
        quantity: <bdi dir="ltr">{formatNumber(count)}</bdi>,
        providerName: <bdi dir="auto">{providerName}</bdi>,
    });
}

export function discoveryPendingMessage(
    t: TFunction<"translation">,
    providerName: string,
): JSX.Element {
    return slots(
        t(($) => $.operationNotices.discoveryPending),
        {
            providerName: <bdi dir="auto">{providerName}</bdi>,
        },
    );
}

export function keyCountMessage(
    t: TFunction<"translation">,
    values: {
        readonly total: number;
        readonly shown: number;
        readonly enabled: number;
        readonly hasSearch: boolean;
    },
): JSX.Element {
    const parameters = {
        count: values.total,
        total: formatNumber(values.total),
        enabled: formatNumber(values.enabled),
    };
    if (!values.hasSearch) return t(($) => $.counts.enabledKeys, parameters);
    return slots(
        t(($) => $.counts.filteredKeys, {
            count: values.total,
        }),
        {
            total: <bdi dir="ltr">{parameters.total}</bdi>,
            enabled: <bdi dir="ltr">{parameters.enabled}</bdi>,
            shownSentence: t(($) => $.counts.shownKeys, {
                count: values.shown,
                shown: formatNumber(values.shown),
            }),
        },
    );
}
export function requestSummaryMessage(
    t: TFunction<"translation">,
    total: number,
    peak: {
        readonly requests: number;
        readonly day: string;
    } | null,
): JSX.Element {
    if (peak === null)
        return t(($) => $.counts.requests, {
            count: total,
            total: formatNumber(total),
        });
    return slots(
        t(($) => $.counts.requestsPeak, {
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
            return slots(
                translation()(($) => $.quotaMessages.counter),
                {
                    used: <bdi dir="ltr">{args[2].used}</bdi>,
                    limit: <bdi dir="ltr">{args[2].limit}</bdi>,

                    unit: isolated,
                },
            );
        case "remaining":
            return slots(
                translation()(($) => $.quotaMessages.remaining),
                {
                    remaining: <bdi dir="ltr">{args[2].remaining}</bdi>,

                    unit: isolated,
                },
            );
        case "atReset":
            return slots(
                translation()(($) => $.quotaMessages.atReset),
                {
                    distance: args[2].distance,

                    instant: isolated,
                },
            );
        case "observed":
            return slots(
                translation()(($) => $.quotaMessages.observed),
                {
                    distance: args[2].distance,

                    instant: isolated,
                },
            );
        case "dateReset":
            return slots(
                translation()(($) => $.quotaMessages.dateReset),
                {
                    date: isolated,
                },
            );
        case "quotaObserved":
            return slots(
                translation()(($) => $.quotaMessages.quotaObserved),
                {
                    instant: isolated,
                },
            );
        case "models":
            return slots(
                translation()(($) => $.quotaMessages.models),
                {
                    identifiers: isolated,
                },
            );
        case "resets":
            return slots(
                translation()(($) => $.quotaMessages.resets),
                {
                    instant: isolated,
                },
            );
    }
}

export function editNameMessage(name: string): JSX.Element {
    return slots(
        translation()(($) => $.entityMessages.editName),
        {
            name: <bdi dir="auto">{name}</bdi>,
        },
    );
}

export function editIdentifierMessage(identifier: string): JSX.Element {
    return slots(
        translation()(($) => $.entityMessages.editIdentifier),
        {
            identifier: <bdi dir="ltr">{identifier}</bdi>,
        },
    );
}

export function deleteKeyMessage(t: TFunction<"translation">, name: string): JSX.Element {
    return slots(
        t(($) => $.entityMessages.deleteKey),
        {
            name: <bdi dir="auto">{name}</bdi>,
        },
    );
}

export function deleteLastKeyMessage(name: string): JSX.Element {
    return slots(
        translation()(($) => $.entityMessages.deleteLastKey),
        {
            name: <bdi dir="auto">{name}</bdi>,
        },
    );
}

export function deleteModelMessage(identifier: string): JSX.Element {
    return slots(
        translation()(($) => $.entityMessages.deleteModel),
        {
            identifier: <bdi dir="ltr">{identifier}</bdi>,
        },
    );
}

export function deleteProviderMessage(name: string): JSX.Element {
    return slots(
        translation()(($) => $.entityMessages.deleteProvider),
        {
            name: <bdi dir="auto">{name}</bdi>,
        },
    );
}

export function switchPresetMessage(name: string): JSX.Element {
    return slots(
        translation()(($) => $.entityMessages.switchPreset),
        {
            name: <bdi dir="auto">{name}</bdi>,
        },
    );
}

export function noKeysMatchMessage(query: string): JSX.Element {
    return slots(
        translation()(($) => $.entityMessages.noKeysMatch),
        {
            query: <bdi dir="auto">{query}</bdi>,
        },
    );
}

export function noProvidersMatchMessage(query: string): JSX.Element {
    return slots(
        translation()(($) => $.entityMessages.noProvidersMatch),
        {
            query: <bdi dir="auto">{query}</bdi>,
        },
    );
}

export function noModelsMatchMessage(query: string): JSX.Element {
    return slots(
        translation()(($) => $.entityMessages.noModelsMatch),
        {
            query: <bdi dir="auto">{query}</bdi>,
        },
    );
}

export function noProviderModelsMatchMessage(query: string): JSX.Element {
    return slots(
        translation()(($) => $.entityMessages.noProviderModelsMatch),
        {
            query: <bdi dir="auto">{query}</bdi>,
        },
    );
}

export function missingPathMessage(path: string): JSX.Element {
    return slots(
        translation()(($) => $.entityMessages.missingPath),
        {
            path: <bdi dir="ltr">{path}</bdi>,
        },
    );
}

export function aliasCreatedMessage(identifier: string): JSX.Element {
    return slots(
        translation()(($) => $.entityMessages.aliasCreated),
        {
            identifier: <bdi dir="ltr">{identifier}</bdi>,
        },
    );
}

export function addAliasMessage(identifier: string): JSX.Element {
    return slots(
        translation()(($) => $.entityMessages.addAlias),
        {
            identifier: <bdi dir="ltr">{identifier}</bdi>,
        },
    );
}

export function poolMembersMessage(count: number, prefix: string): JSX.Element {
    return slots(
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
    return slots(
        translation()(($) => $.routingMessages.direct),
        {
            provider: <bdi dir="auto">{provider}</bdi>,
            model: <bdi dir="ltr">{model}</bdi>,
        },
    );
}
export function missingDirectTargetMessage(id: number): JSX.Element {
    return slots(
        translation()(($) => $.routingMessages.directMissing),
        {
            id: <bdi dir="ltr">{formatNumber(id)}</bdi>,
        },
    );
}
export function poolTargetMessage(pool: string, model: string): JSX.Element {
    return slots(
        translation()(($) => $.routingMessages.pool),
        {
            pool: <bdi dir="auto">{pool}</bdi>,
            model: <bdi dir="ltr">{model}</bdi>,
        },
    );
}
export function publicIdsMessage(identifiers: string): JSX.Element {
    return slots(
        translation()(($) => $.routingMessages.publicIds),
        {
            identifiers: <bdi dir="ltr">{identifiers}</bdi>,
        },
    );
}
export function reconnectMessage(isEnabled: boolean, href: string): JSX.Element {
    return slots(
        isEnabled
            ? translation()(($) => $.routingMessages.reconnect)
            : translation()(($) => $.routingMessages.reconnectDisabled),
        {
            editLink: (
                <AppLink
                    href={href}
                    class="font-medium text-accent underline underline-offset-4"
                >
                    {translation()(($) => $.screens.providerDetailScreen.editPage)}
                </AppLink>
            ),
        },
    );
}

export function networkTitleMessage(
    t: TFunction<"translation">,
    kind: "unreachable" | "mixed-content" | "blocked" | "no-answer",
    baseUrl: string,
): JSX.Element {
    const template =
        kind === "unreachable"
            ? t(($) => $.diagnosis.nothingAnsweredAtBaseUrl)
            : kind === "mixed-content"
              ? t(($) => $.diagnosis.anHTTPSPageCannotCallTheHTTPAddressBaseUrl)
              : kind === "blocked"
                ? t(($) => $.diagnosis.theGatewayAnsweredButTheBrowserBlockedTheResponseFromBaseUrl)
                : t(($) => $.diagnosis.noAnswerFromBaseUrl);
    return slots(template, {
        baseUrl: <bdi dir="ltr">{baseUrl}</bdi>,
    });
}
export function corsRecoveryMessage(isConfirmed: boolean, origin: string): JSX.Element {
    const t = translation();
    return slots(
        isConfirmed
            ? t(
                  ($) =>
                      $.diagnosis
                          .addOriginToSettingInThatInstancesConfigurationFileAndRestartTheGatewayServing,
              )
            : t(
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
    return slots(
        translation()(($) => $.routingMessages.ownerDefault[kind]),
        {
            name: <bdi dir="auto">{name}</bdi>,
        },
    );
}
export function modelAccessMessage(identifier: string): JSX.Element {
    return slots(
        translation()(($) => $.routingMessages.modelAccess),
        {
            identifier: <bdi dir="ltr">{identifier}</bdi>,
        },
    );
}
export function directAccessTargetMessage(id: number): JSX.Element {
    return slots(
        translation()(($) => $.routingMessages.directTarget),
        {
            id: <bdi dir="ltr">{formatNumber(id)}</bdi>,
        },
    );
}
export function poolAccessTargetMessage(identifier: string): JSX.Element {
    return slots(
        translation()(($) => $.routingMessages.poolTarget),
        {
            identifier: <bdi dir="ltr">{identifier}</bdi>,
        },
    );
}

export function keyEndpointMessage(overviewHref: string): JSX.Element {
    return slots(
        translation()(($) => $.linkedMessages.keyEndpoint),
        {
            overviewLink: (
                <AppLink
                    href={overviewHref}
                    class="font-medium text-accent underline underline-offset-4"
                >
                    {translation()(($) => $.screens.apiKeyDetailScreen.overview)}
                </AppLink>
            ),
        },
    );
}
export function servingKeyMessage(keysHref: string): JSX.Element {
    return slots(
        translation()(($) => $.linkedMessages.servingKey),
        {
            keysLink: (
                <AppLink
                    href={keysHref}
                    class="font-medium text-accent underline underline-offset-4"
                >
                    {translation()(($) => $.screens.homeScreen.apiKeys)}
                </AppLink>
            ),
        },
    );
}
export function vanishedModelMessage(providerHref: string, claimHref: string): JSX.Element {
    return slots(
        translation()(($) => $.linkedMessages.vanished),
        {
            discoverLink: (
                <AppLink
                    href={providerHref}
                    class="font-medium text-accent underline underline-offset-4"
                >
                    {translation()(($) => $.linkedMessages.discoverLabel)}
                </AppLink>
            ),
            claimLink: (
                <AppLink
                    href={claimHref}
                    class="font-medium text-accent underline underline-offset-4"
                >
                    {translation()(($) => $.linkedMessages.claimLabel)}
                </AppLink>
            ),
        },
    );
}
export function inventoryAttentionMessage(
    name: string,
    reason: "no-models" | "no-ready-models",
): JSX.Element {
    const template =
        reason === "no-models"
            ? translation()(($) => $.linkedMessages.inventoryEmpty)
            : translation()(($) => $.linkedMessages.inventoryUnready);
    return slots(template, {
        name: (
            <bdi
                dir="auto"
                class="font-medium text-ink"
            >
                {name}
            </bdi>
        ),
    });
}
export function requestChartPointMessage(
    t: TFunction<"translation">,
    day: string,
    count: number,
    quantity: string,
): {
    readonly accessibleName: string;
    readonly content: JSX.Element;
} {
    const template = t(($) => $.linkedMessages.chartPoint, {
        count,
    });
    return {
        accessibleName: template.replace("[[day]]", day).replace("[[quantity]]", quantity),
        content: slots(template, {
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
