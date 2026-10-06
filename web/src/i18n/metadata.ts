import type english from "./catalogs/en";

type SemanticKey<T> = T extends string
    ? never
    : {
          [K in keyof T & string]: T[K] extends string
              ? K extends `${infer P}_ordinal_${"zero" | "one" | "two" | "few" | "many" | "other"}`
                  ? P
                  : K extends `${infer P}_${"zero" | "one" | "two" | "few" | "many" | "other"}`
                    ? P
                    : K
              : `${K}.${SemanticKey<T[K]>}`;
      }[keyof T & string];
export type MessagePath = SemanticKey<typeof english>;
export type PluralContract = {
    readonly path: string;
    readonly kind: "cardinal" | "ordinal";
};
export type RichContract = {
    readonly path: string;
    readonly slots: readonly string[];
};
/** Semantic plural paths, independent of a locale's grammatical category set. */
export const pluralMessages = [
    {
        path: "linkedMessages.chartPoint",
        kind: "cardinal",
    },
    {
        path: "routingMessages.directRoutes",
        kind: "cardinal",
    },
    {
        path: "routingMessages.poolMembers",
        kind: "cardinal",
    },
    {
        path: "quotaMessages.day",
        kind: "cardinal",
    },
    {
        path: "quotaMessages.hour",
        kind: "cardinal",
    },
    {
        path: "quotaMessages.minute",
        kind: "cardinal",
    },
    {
        path: "quotaMessages.second",
        kind: "cardinal",
    },

    {
        path: "counts.models",
        kind: "cardinal",
    },
    {
        path: "counts.filteredModels",
        kind: "cardinal",
    },
    {
        path: "counts.enabledKeys",
        kind: "cardinal",
    },
    {
        path: "counts.shownKeys",
        kind: "cardinal",
    },
    {
        path: "counts.filteredKeys",
        kind: "cardinal",
    },
    {
        path: "counts.providerDeleted",
        kind: "cardinal",
    },
    {
        path: "counts.requests",
        kind: "cardinal",
    },
    {
        path: "counts.requestsPeak",
        kind: "cardinal",
    },

    {
        path: "notices.discovered",
        kind: "cardinal",
    },
    {
        path: "notices.manualUntouched",
        kind: "cardinal",
    },
    {
        path: "notices.vanished",
        kind: "cardinal",
    },
    {
        path: "models.discoveredCount",
        kind: "cardinal",
    },
] as const satisfies readonly (PluralContract & {
    path: MessagePath;
})[];
export const richMessages = [
    {
        path: "linkedMessages.keyEndpoint",
        slots: [
            "overviewLink",
        ],
    },
    {
        path: "linkedMessages.servingKey",
        slots: [
            "keysLink",
        ],
    },
    {
        path: "linkedMessages.vanished",
        slots: [
            "discoverLink",
            "claimLink",
        ],
    },
    {
        path: "linkedMessages.chartPoint",
        slots: [
            "day",
            "quantity",
        ],
    },
    {
        path: "linkedMessages.inventoryEmpty",
        slots: [
            "name",
        ],
    },
    {
        path: "linkedMessages.inventoryUnready",
        slots: [
            "name",
        ],
    },

    {
        path: "routingMessages.directTarget",
        slots: [
            "id",
        ],
    },
    {
        path: "routingMessages.poolTarget",
        slots: [
            "identifier",
        ],
    },
    {
        path: "diagnosis.nothingAnsweredAtBaseUrl",
        slots: [
            "baseUrl",
        ],
    },
    {
        path: "diagnosis.anHTTPSPageCannotCallTheHTTPAddressBaseUrl",
        slots: [
            "baseUrl",
        ],
    },
    {
        path: "diagnosis.theGatewayAnsweredButTheBrowserBlockedTheResponseFromBaseUrl",
        slots: [
            "baseUrl",
        ],
    },
    {
        path: "diagnosis.noAnswerFromBaseUrl",
        slots: [
            "baseUrl",
        ],
    },
    {
        path: "diagnosis.addOriginToSettingInThatInstancesConfigurationFileAndRestartTheGatewayServing",
        slots: [
            "origin",
            "setting",
        ],
    },
    {
        path: "diagnosis.eitherNothingIsListeningAtThatAddressOrTheBrowserBlockedTheResponseBefore",
        slots: [
            "origin",
            "setting",
        ],
    },

    {
        path: "routingMessages.poolMembers",
        slots: [
            "quantity",
            "prefix",
        ],
    },
    {
        path: "routingMessages.directMissing",
        slots: [
            "id",
        ],
    },
    {
        path: "routingMessages.direct",
        slots: [
            "provider",
            "model",
        ],
    },
    {
        path: "routingMessages.pool",
        slots: [
            "pool",
            "model",
        ],
    },
    {
        path: "routingMessages.publicIds",
        slots: [
            "identifiers",
        ],
    },
    {
        path: "routingMessages.modelAccess",
        slots: [
            "identifier",
        ],
    },
    {
        path: "routingMessages.ownerDefault.provider",
        slots: [
            "name",
        ],
    },
    {
        path: "routingMessages.ownerDefault.pool",
        slots: [
            "name",
        ],
    },
    {
        path: "routingMessages.reconnect",
        slots: [
            "editLink",
        ],
    },
    {
        path: "routingMessages.reconnectDisabled",
        slots: [
            "editLink",
        ],
    },

    {
        path: "entityMessages.editName",
        slots: [
            "name",
        ],
    },
    {
        path: "entityMessages.editIdentifier",
        slots: [
            "identifier",
        ],
    },
    {
        path: "entityMessages.deleteKey",
        slots: [
            "name",
        ],
    },
    {
        path: "entityMessages.deleteLastKey",
        slots: [
            "name",
        ],
    },
    {
        path: "entityMessages.deleteModel",
        slots: [
            "identifier",
        ],
    },
    {
        path: "entityMessages.deleteProvider",
        slots: [
            "name",
        ],
    },
    {
        path: "entityMessages.switchPreset",
        slots: [
            "name",
        ],
    },
    {
        path: "entityMessages.noKeysMatch",
        slots: [
            "query",
        ],
    },
    {
        path: "entityMessages.noProvidersMatch",
        slots: [
            "query",
        ],
    },
    {
        path: "entityMessages.noModelsMatch",
        slots: [
            "query",
        ],
    },
    {
        path: "entityMessages.noProviderModelsMatch",
        slots: [
            "query",
        ],
    },
    {
        path: "entityMessages.missingPath",
        slots: [
            "path",
        ],
    },
    {
        path: "entityMessages.aliasCreated",
        slots: [
            "identifier",
        ],
    },
    {
        path: "entityMessages.addAlias",
        slots: [
            "identifier",
        ],
    },

    {
        path: "quotaMessages.counter",
        slots: [
            "used",
            "limit",

            "unit",
        ],
    },
    {
        path: "quotaMessages.remaining",
        slots: [
            "remaining",

            "unit",
        ],
    },
    {
        path: "quotaMessages.dateReset",
        slots: [
            "date",
        ],
    },
    {
        path: "quotaMessages.atReset",
        slots: [
            "distance",

            "instant",
        ],
    },
    {
        path: "quotaMessages.observed",
        slots: [
            "distance",

            "instant",
        ],
    },
    {
        path: "quotaMessages.quotaObserved",
        slots: [
            "instant",
        ],
    },
    {
        path: "quotaMessages.models",
        slots: [
            "identifiers",
        ],
    },
    {
        path: "quotaMessages.resets",
        slots: [
            "instant",
        ],
    },

    {
        path: "counts.filteredKeys",
        slots: [
            "total",
            "enabled",

            "shownSentence",
        ],
    },
    {
        path: "counts.requestsPeak",
        slots: [
            "total",
            "peak",

            "day",
        ],
    },
    {
        path: "operationNotices.discoveryPending",
        slots: [
            "providerName",
        ],
    },
    {
        path: "notices.savedName",
        slots: [
            "name",
        ],
    },
    {
        path: "notices.discovered",
        slots: [
            "quantity",
            "providerName",
        ],
    },
    {
        path: "notices.nothingDiscovered",
        slots: [
            "providerName",
        ],
    },
    {
        path: "rich.connected",
        slots: [
            "baseUrl",
        ],
    },
    {
        path: "rich.setupToken",
        slots: [
            "setting",
        ],
    },
    {
        path: "rich.setupCors",
        slots: [
            "setting",
        ],
    },
] as const satisfies readonly (RichContract & {
    path: MessagePath;
})[];
