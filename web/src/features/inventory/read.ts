import type { ManagementApi } from "/src/management-api/client";
import type {
    ListApiKeysResponse,
    ListModelRoutesResponse,
    ListModelsRequest,
    ListModelsResponse,
    ListProviderDefinitionsResponse,
    ListProviderPoolsResponse,
    ListProvidersResponse,
    ModelResponse,
    ModelRouteResponse,
    ProviderPoolResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import type { ReadCall } from "/src/screen-state/read";
import { readAll } from "/src/screen-state/read";

/**
 * The entity listings' named read calls, the way the usage domain names its
 * reads (`readUsageEntities`): a screen's cycle spells its subject as intent
 * instead of hand-writing the arrow over the client's method, and screens
 * reading several listings compose the named calls under one `readAll`.
 */

/** The providers listing, as one read call run under the cycle's signal. */
export function readProviders(
    api: Pick<ManagementApi, "listProviders">,
): ReadCall<ListProvidersResponse> {
    return {
        run: (cycle) => api.listProviders(cycle),
    };
}

/** The provider-definitions listing, as one read call run under the cycle's signal. */
export function readProviderDefinitions(
    api: Pick<ManagementApi, "listProviderDefinitions">,
): ReadCall<ListProviderDefinitionsResponse> {
    return {
        run: (cycle) => api.listProviderDefinitions(cycle),
    };
}

/** The models listing for the given request, as one read call run under the cycle's signal. */
export function readModels(
    api: Pick<ManagementApi, "listModels">,
    request: ListModelsRequest,
): ReadCall<ListModelsResponse> {
    return {
        run: (cycle) => api.listModels(request, cycle),
    };
}

/** The serving keys listing, as one read call run under the cycle's signal. */
export function readApiKeys(
    api: Pick<ManagementApi, "listApiKeys">,
): ReadCall<ListApiKeysResponse> {
    return {
        run: (cycle) => api.listApiKeys(cycle),
    };
}

/** The public routes listing, as one read call run under the cycle's signal. */
export function readModelRoutes(
    api: Pick<ManagementApi, "listModelRoutes">,
): ReadCall<ListModelRoutesResponse> {
    return {
        run: (cycle) => api.listModelRoutes(cycle),
    };
}

/** The provider pools listing, as one read call run under the cycle's signal. */
export function readProviderPools(
    api: Pick<ManagementApi, "listProviderPools">,
): ReadCall<ListProviderPoolsResponse> {
    return {
        run: (cycle) => api.listProviderPools(cycle),
    };
}

/** The entity pair the provider and model screens read alongside their rows. */
export type ProviderCatalog = {
    readonly providers: ReadonlyArray<ProviderResponse>;
    readonly models: ReadonlyArray<ModelResponse>;
};

/**
 * The two entity listings a screen reads beside its subject — the provider
 * and model screens alongside their rows — as one read call: the listings
 * run in parallel under the cycle's signal and answer as the whole
 * `ProviderCatalog` pair, so every cycle that ends successfully leaves both
 * listings loaded. A non-null `providerId` filters the models listing to
 * that provider's rows; `null` reads every model row.
 */
export function readProviderCatalog(
    api: Pick<ManagementApi, "listProviders" | "listModels">,
    providerId: number | null,
): ReadCall<ProviderCatalog> {
    return {
        run: async (signal) => {
            const [providersResult, modelsResult] = await readAll(
                signal,
                readProviders(api),
                readModels(
                    api,
                    providerId === null
                        ? {}
                        : {
                              provider_id: providerId,
                          },
                ),
            );

            return {
                providers: providersResult.providers,
                models: modelsResult.models,
            };
        },
    };
}

/** Reads physical inventory and its authored direct routes without upstream I/O. */
export function readPublishedInventory(
    api: Pick<
        ManagementApi,
        "listProviders" | "listModels" | "listModelRoutes" | "listProviderPools"
    >,
    providerId: number | null,
): ReadCall<
    ProviderCatalog & {
        readonly modelRoutes: ReadonlyArray<ModelRouteResponse>;
        readonly pools: ReadonlyArray<ProviderPoolResponse>;
    }
> {
    return {
        run: async (signal) => {
            const [inventory, routes, pools] = await readAll(
                signal,
                readProviderCatalog(api, providerId),
                readModelRoutes(api),
                readProviderPools(api),
            );
            return {
                ...inventory,
                modelRoutes: routes.model_routes,
                pools: pools.provider_pools,
            };
        },
    };
}
