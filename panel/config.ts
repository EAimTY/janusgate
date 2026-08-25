export const panelConfig = {
    tokenStorageKey: "panel_token",
    tokenExpiryWarningSeconds: 24 * 60 * 60,
    routes: {
        root: "/",
        providers: "/providers",
        models: "/models",
    },
    panelEndpoints: {
        listProviders: "/panel-api/list-providers",
        saveProvider: "/panel-api/save-provider",
        deleteProvider: "/panel-api/delete-provider",
        startProviderOAuth: "/panel-api/start-provider-oauth",
        completeProviderOAuth: "/panel-api/complete-provider-oauth",
        listModels: "/panel-api/list-models",
        saveModel: "/panel-api/save-model",
        deleteModel: "/panel-api/delete-model",
        discoverModels: "/panel-api/discover-models",
    },
} as const;

export type AppRoute =
    | typeof panelConfig.routes.providers
    | typeof panelConfig.routes.models
    | "not-found";
