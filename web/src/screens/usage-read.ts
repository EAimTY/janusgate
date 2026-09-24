import type { ManagementApi } from "/src/management-api/client";
import { ManagementApiError } from "/src/management-api/errors";
import type {
    ApiKeyResponse,
    ModelResponse,
    ProviderResponse,
    QueryUsageRequest,
    QueryUsageResponse,
    UsageBucketKey,
    UsageGroupBy,
} from "/src/management-api/generated/v0";
import { type ReadCall, readAll } from "./read";

/** The entity lists a usage screen reads alongside the accounting rows. */
export type UsageEntities = {
    readonly providers: ReadonlyArray<ProviderResponse>;
    readonly models: ReadonlyArray<ModelResponse>;
    readonly apiKeys: ReadonlyArray<ApiKeyResponse>;
};

/**
 * The three entity listings a screen reads to name entities — the usage
 * screens alongside their rows, the overview for its health counts — as one
 * read call: the listings run in parallel under the cycle's signal and answer
 * as the whole `UsageEntities` triple, so every cycle that ends successfully
 * leaves the labels' resolution lists loaded.
 */
export function readUsageEntities(
    api: Pick<ManagementApi, "listProviders" | "listModels" | "listApiKeys">,
): ReadCall<UsageEntities> {
    return {
        run: async (signal) => {
            const [providersResult, modelsResult, apiKeysResult] = await readAll(
                signal,
                {
                    run: (cycle) => api.listProviders(cycle),
                },
                {
                    run: (cycle) => api.listModels({}, cycle),
                },
                {
                    run: (cycle) => api.listApiKeys(cycle),
                },
            );

            return {
                providers: providersResult.providers,
                models: modelsResult.models,
                apiKeys: apiKeysResult.api_keys,
            };
        },
    };
}

/** The bucket-key field each requested dimension's part rides on. */
function bucketKeyFieldOf(dimension: UsageGroupBy): keyof UsageBucketKey {
    switch (dimension) {
        case "day":
            return "day";
        case "model":
            return "model_id";
        case "provider":
            return "provider_id";
        case "api_key":
            return "api_key_id";
        case "endpoint_type":
            return "endpoint_type";
        case "outcome":
            return "outcome";
        case "is_stream":
            return "is_stream";
        case "http_status":
            return "http_status";
    }
}

/**
 * Runs one `usage:query` and proves its answer inside the read cycle: a
 * bucket key carries exactly the request's dimensions, so a key missing one
 * is a contract failure — the same `ManagementApiError` a shape mismatch
 * raises — thrown while the cycle can still state its diagnosis, never at
 * render or stats time, where it would blank the screen or die as an
 * unhandled rejection (README §6.4). A key part that is `null` — the
 * `NULL` status bucket — is present: only an absent field violates.
 */
export function readUsageBuckets(
    api: Pick<ManagementApi, "queryUsage">,
    request: QueryUsageRequest,
): ReadCall<QueryUsageResponse> {
    return {
        action: "query-usage",
        run: async (signal) => {
            const result = await api.queryUsage(request, signal);

            for (const bucket of result.buckets) {
                for (const dimension of request.group_by) {
                    const field = bucketKeyFieldOf(dimension);

                    if (bucket.key[field] === undefined) {
                        throw new ManagementApiError(
                            "contract",
                            `Unexpected management API response: the bucket key carries no ${field}`,
                        );
                    }
                }
            }

            return result;
        },
    };
}
