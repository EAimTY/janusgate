import type { ManagementApi } from "/src/management-api/client";
import { ContractViolationError } from "/src/management-api/errors";
import type {
    ApiKeyResponse,
    ModelResponse,
    ProviderResponse,
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
export function readUsageEntities(api: ManagementApi): ReadCall<UsageEntities> {
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

type UsageLabel = {
    /** The current name, or what kind of row is gone when it is no longer listed. */
    readonly text: string;
    /** False when the id is no longer listed — the row was deleted after the request was recorded. */
    readonly isListed: boolean;
};

function resolveLabel(
    names: ReadonlyArray<{
        readonly id: number;
        readonly name: string;
    }>,
    id: number,
    missingText: string,
): UsageLabel {
    const row = names.find((candidate) => candidate.id === id);

    return row === undefined
        ? {
              text: missingText,
              isListed: false,
          }
        : {
              text: row.name,
              isListed: true,
          };
}

/** A request's model as its current public ID, or as a deleted model. */
export function usageModelLabel(entities: UsageEntities, modelId: number): UsageLabel {
    return resolveLabel(
        entities.models.map((row) => ({
            id: row.id,
            name: row.public_id,
        })),
        modelId,
        "Deleted model",
    );
}

/** A request's provider as its current name, or as a deleted provider. */
export function usageProviderLabel(entities: UsageEntities, providerId: number): UsageLabel {
    return resolveLabel(entities.providers, providerId, "Deleted provider");
}

/** A request's API key as its current name, or as a deleted key. */
export function usageApiKeyLabel(entities: UsageEntities, apiKeyId: number): UsageLabel {
    return resolveLabel(entities.apiKeys, apiKeyId, "Deleted key");
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
 * the day verbatim, ids resolved through the entity lists, the enum
 * dimensions through their vocabularies. The `NULL` status bucket reads as
 * an em dash — the wire's `null` is a real bucket, never a missing one.
 */
export function usageDimensionLabel(
    dimension: UsageGroupBy,
    key: UsageBucketKey,
    entities: UsageEntities,
): string {
    switch (dimension) {
        case "day":
            return bucketKeyField(key.day, "day");

        case "model":
            return usageModelLabel(entities, bucketKeyField(key.model_id, "model_id")).text;

        case "provider":
            return usageProviderLabel(entities, bucketKeyField(key.provider_id, "provider_id"))
                .text;

        case "api_key":
            return usageApiKeyLabel(entities, bucketKeyField(key.api_key_id, "api_key_id")).text;

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
