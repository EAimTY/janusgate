import type { ManagementApi } from "/src/management-api/client";

/**
 * Test support the screen suites share: the full management API stub their
 * mounts run against and the promise drain their screen exchanges rely on.
 */

/** Rejects the way every route a screen test did not expect must: loudly. */
function refusedRoute(route: keyof ManagementApi): Promise<never> {
    return Promise.reject(new Error(`route not expected in test: ${route}`));
}

/**
 * The full route table a screen test mounts against: the given overrides
 * answer, and every route they leave out rejects with a refusal naming the
 * route, so an unexpected call fails the test loudly instead of hanging it.
 * The table is typed against the real `ManagementApi`, so a route the
 * gateway adds or renames breaks this module — and only this module — at
 * compile time.
 */
export function stubManagementApi(overrides: Partial<ManagementApi>): ManagementApi {
    return {
        listProviderDefinitions:
            overrides.listProviderDefinitions ?? (() => refusedRoute("listProviderDefinitions")),
        createProviderDefinition:
            overrides.createProviderDefinition ?? (() => refusedRoute("createProviderDefinition")),
        updateProviderDefinition:
            overrides.updateProviderDefinition ?? (() => refusedRoute("updateProviderDefinition")),
        listProviders: overrides.listProviders ?? (() => refusedRoute("listProviders")),
        createProvider: overrides.createProvider ?? (() => refusedRoute("createProvider")),
        updateProvider: overrides.updateProvider ?? (() => refusedRoute("updateProvider")),
        deleteProvider: overrides.deleteProvider ?? (() => refusedRoute("deleteProvider")),
        startProviderOauth:
            overrides.startProviderOauth ?? (() => refusedRoute("startProviderOauth")),
        completeProviderOauth:
            overrides.completeProviderOauth ?? (() => refusedRoute("completeProviderOauth")),
        reauthorizeProvider:
            overrides.reauthorizeProvider ?? (() => refusedRoute("reauthorizeProvider")),
        listModels: overrides.listModels ?? (() => refusedRoute("listModels")),
        discoverModels: overrides.discoverModels ?? (() => refusedRoute("discoverModels")),
        updateModel: overrides.updateModel ?? (() => refusedRoute("updateModel")),
        createModel: overrides.createModel ?? (() => refusedRoute("createModel")),
        deleteModel: overrides.deleteModel ?? (() => refusedRoute("deleteModel")),
        listUsage: overrides.listUsage ?? (() => refusedRoute("listUsage")),
        queryUsage: overrides.queryUsage ?? (() => refusedRoute("queryUsage")),
        listApiKeys: overrides.listApiKeys ?? (() => refusedRoute("listApiKeys")),
        createApiKey: overrides.createApiKey ?? (() => refusedRoute("createApiKey")),
        updateApiKey: overrides.updateApiKey ?? (() => refusedRoute("updateApiKey")),
        deleteApiKey: overrides.deleteApiKey ?? (() => refusedRoute("deleteApiKey")),
    };
}

/** Drains the pending promises and scheduled work of one screen exchange. */
export async function settle(): Promise<void> {
    for (let round = 0; round < 10; round += 1) {
        await new Promise<void>((resolve) => setTimeout(resolve, 0));
    }
}
