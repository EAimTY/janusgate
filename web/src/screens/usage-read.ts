import type { ManagementApi } from "/src/management-api/client";
import { ManagementApiError } from "/src/management-api/errors";
import type {
    ApiKeyResponse,
    ListUsageRequest,
    ListUsageResponse,
    ModelResponse,
    ProviderResponse,
    QueryUsageRequest,
    QueryUsageResponse,
    UsageFilter,
} from "/src/management-api/generated/v0";
import { type ReadCall, readAll, readApiKeys, readModels, readProviders } from "./read";
import { DIMENSION_KEY_FIELD, missingBucketKeyFieldMessage } from "./usage-labels";
import { USAGE_PAGE_SIZE } from "./usage-pagination";

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
                readProviders(api),
                readModels(api, {}),
                readApiKeys(api),
            );

            return {
                providers: providersResult.providers,
                models: modelsResult.models,
                apiKeys: apiKeysResult.api_keys,
            };
        },
    };
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
                    const field = DIMENSION_KEY_FIELD[dimension];

                    if (bucket.key[field] === undefined) {
                        throw new ManagementApiError(
                            "contract",
                            `Unexpected management API response: ${missingBucketKeyFieldMessage(field)}`,
                        );
                    }
                }
            }

            return result;
        },
    };
}

/**
 * One `usage:list` request: the page size, the cursor when the stream
 * continues, and the filter only when something is chosen — an absent filter
 * is the wire's own spelling of "every row", so the unfiltered stream sends
 * exactly what it always did.
 */
export function usageListRequest(filter: UsageFilter, before: number | null): ListUsageRequest {
    return {
        limit: USAGE_PAGE_SIZE,
        ...(before === null
            ? {}
            : {
                  before,
              }),
        ...(Object.keys(filter).length === 0
            ? {}
            : {
                  filter,
              }),
    };
}

/**
 * The usage stream's listing, as one read call run under the cycle's signal:
 * the carried action names the call a `usage:list`, so its failure diagnoses
 * as that call's, the way every named read call here does.
 */
export function readUsagePage(
    api: Pick<ManagementApi, "listUsage">,
    request: ListUsageRequest,
): ReadCall<ListUsageResponse> {
    return {
        action: "list-usage",
        run: (cycle) => api.listUsage(request, cycle),
    };
}
