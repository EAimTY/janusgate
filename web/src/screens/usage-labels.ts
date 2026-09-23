import type { ManagementApi } from "/src/management-api/client";
import { ContractViolationError, ManagementApiError } from "/src/management-api/errors";
import type {
    ApiKeyResponse,
    ModelResponse,
    ProviderResponse,
    QueryUsageRequest,
    QueryUsageResponse,
    UsageBucketKey,
    UsageGroupBy,
} from "/src/management-api/generated/v0";
import { apiLogEndpointLabel, apiLogOutcomeLabel } from "/src/management-api/vocabulary";
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

type UsageLabel = {
    /** The current name, or what kind of row is gone when it is no longer listed. */
    readonly text: string;
    /** False when the id is no longer listed — the row was deleted after the request was recorded. */
    readonly isListed: boolean;
};

/** The usage-domain entity lists indexed by row id, ready for O(1) lookups. */
export type UsageEntityIndex = {
    readonly providersById: ReadonlyMap<number, ProviderResponse>;
    readonly modelsById: ReadonlyMap<number, ModelResponse>;
    readonly apiKeysById: ReadonlyMap<number, ApiKeyResponse>;
};

/**
 * Maps the rows by their id. Row ids are unique by contract, so each id
 * answers exactly one row.
 */
function indexById<
    TRow extends {
        readonly id: number;
    },
>(rows: ReadonlyArray<TRow>): ReadonlyMap<number, TRow> {
    return new Map(
        rows.map((row) => [
            row.id,
            row,
        ]),
    );
}

/**
 * Indexes the entity lists once per entities change — a memo over the
 * entities signal, never per rendered cell — so the label resolvers below
 * answer every id in constant time.
 */
export function indexUsageEntities(entities: UsageEntities): UsageEntityIndex {
    return {
        providersById: indexById(entities.providers),
        modelsById: indexById(entities.models),
        apiKeysById: indexById(entities.apiKeys),
    };
}

function resolveLabel(name: string | undefined, missingText: string): UsageLabel {
    return name === undefined
        ? {
              text: missingText,
              isListed: false,
          }
        : {
              text: name,
              isListed: true,
          };
}

/** A request's model as its current public ID, or as a deleted model. */
export function usageModelLabel(
    modelsById: ReadonlyMap<number, ModelResponse>,
    modelId: number,
): UsageLabel {
    return resolveLabel(modelsById.get(modelId)?.public_id, "Deleted model");
}

/** A request's provider as its current name, or as a deleted provider. */
export function usageProviderLabel(
    providersById: ReadonlyMap<number, ProviderResponse>,
    providerId: number,
): UsageLabel {
    return resolveLabel(providersById.get(providerId)?.name, "Deleted provider");
}

/** A request's API key as its current name, or as a deleted key. */
export function usageApiKeyLabel(
    apiKeysById: ReadonlyMap<number, ApiKeyResponse>,
    apiKeyId: number,
): UsageLabel {
    return resolveLabel(apiKeysById.get(apiKeyId)?.name, "Deleted key");
}

/**
 * Reads one bucket key field the request's own dimension list guarantees:
 * a key carries exactly the requested dimensions, so an absent field is a
 * contract violation, not a blank to render past.
 */
export function bucketKeyField<TValue>(value: TValue | undefined, field: string): TValue {
    if (value === undefined) {
        throw new ContractViolationError(`the bucket key carries no ${field}`);
    }

    return value;
}

/**
 * One requested dimension's part of a bucket key, in the operator's words:
 * the day verbatim, ids resolved through the entity index, the enum
 * dimensions through their vocabularies. The `NULL` status bucket reads as
 * an em dash — the wire's `null` is a real bucket, never a missing one.
 */
export function usageDimensionLabel(
    dimension: UsageGroupBy,
    key: UsageBucketKey,
    index: UsageEntityIndex,
): string {
    switch (dimension) {
        case "day":
            return bucketKeyField(key.day, "day");

        case "model":
            return usageModelLabel(index.modelsById, bucketKeyField(key.model_id, "model_id")).text;

        case "provider":
            return usageProviderLabel(
                index.providersById,
                bucketKeyField(key.provider_id, "provider_id"),
            ).text;

        case "api_key":
            return usageApiKeyLabel(index.apiKeysById, bucketKeyField(key.api_key_id, "api_key_id"))
                .text;

        case "endpoint_type":
            return apiLogEndpointLabel(bucketKeyField(key.endpoint_type, "endpoint_type"));

        case "outcome":
            return apiLogOutcomeLabel(bucketKeyField(key.outcome, "outcome"));

        case "is_stream":
            return bucketKeyField(key.is_stream, "is_stream") ? "Streamed" : "Non-streamed";

        case "http_status": {
            const status = bucketKeyField(key.http_status, "http_status");

            return status === null ? "—" : String(status);
        }
    }
}
