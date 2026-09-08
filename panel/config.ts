import { PANEL_API_ENDPOINTS } from "/panel-api/config";

export const panelConfig = {
    tokenStorageKey: "panel_token",
    tokenExpiryWarningSeconds: 24 * 60 * 60,
    routes: {
        root: "/",
        providers: "/providers",
        models: "/models",
    },
    panelEndpoints: PANEL_API_ENDPOINTS,
} as const;

export type AppRoute =
    | typeof panelConfig.routes.providers
    | typeof panelConfig.routes.models
    | "not-found";
