import { ContractViolationError } from "/src/management-api/errors";
import type {
    ApiKeyResponse,
    ModelResponse,
    ProviderResponse,
    UsageBucketKey,
    UsageGroupBy,
} from "/src/management-api/generated/v0";
import { indexRowsById } from "/src/management-api/serving";
import { apiLogEndpointLabel, apiLogOutcomeLabel } from "/src/management-api/vocabulary";
import type { UsageEntities } from "./usage-read";

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
 * Indexes the entity lists once per entities change — a memo over the
 * entities signal, never per rendered cell — so the label resolvers below
 * answer every id in constant time.
 */
export function indexUsageEntities(entities: UsageEntities): UsageEntityIndex {
    return {
        providersById: indexRowsById(entities.providers),
        modelsById: indexRowsById(entities.models),
        apiKeysById: indexRowsById(entities.apiKeys),
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
