import { ContractViolationError } from "/src/management-api/errors";
import type {
    ApiKeyResponse,
    ModelResponse,
    ProviderResponse,
    UsageBucket,
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
 * The one sentence a missing bucket-key field is stated with, whatever error
 * domain throws it: the read proves every requested dimension is present
 * while the cycle can still state its diagnosis, and the render asserts the
 * same invariant — two deliberate error domains over one shared sentence.
 */
export function missingBucketKeyFieldMessage(field: string): string {
    return `the bucket key carries no ${field}`;
}

/**
 * The bucket-key field each requested dimension's part rides on: the one
 * pairing the read's per-key proof (`readUsageBuckets`) and the render's
 * per-cell assert (`usageDimensionLabel`) both consume, so a dimension and
 * its wire field cannot drift apart. The literal-preserving type keeps every
 * entry's field exact, so a dimension narrowed by the dispatch reads its key
 * part with precisely the value type its renderer expects.
 */
export const DIMENSION_KEY_FIELD = {
    day: "day",
    model: "model_id",
    provider: "provider_id",
    api_key: "api_key_id",
    endpoint_type: "endpoint_type",
    outcome: "outcome",
    is_stream: "is_stream",
    http_status: "http_status",
} as const satisfies Readonly<Record<UsageGroupBy, keyof UsageBucketKey>>;

/**
 * Reads one bucket key field the request's own dimension list guarantees:
 * a key carries exactly the requested dimensions, so an absent field is a
 * contract violation, not a blank to render past.
 */
export function bucketKeyField<TValue>(value: TValue | undefined, field: string): TValue {
    if (value === undefined) {
        throw new ContractViolationError(missingBucketKeyFieldMessage(field));
    }

    return value;
}

/**
 * The day series as every usage display shows it: newest day first, with the
 * gateway's own order kept within equal days — the comparison answers 0 for
 * equal days, so the stable sort (ES2019+) preserves the answer's order
 * inside a day and invents no ordering the gateway did not state. The day
 * part is the invariant `readUsageBuckets` proved inside the read cycle, so
 * the comparator asserts it exactly as the key cells do.
 */
export function daysNewestFirst(buckets: ReadonlyArray<UsageBucket>): Array<UsageBucket> {
    return [
        ...buckets,
    ].sort((left, right) => {
        const leftDay = bucketKeyField(left.key.day, "day");
        const rightDay = bucketKeyField(right.key.day, "day");

        return leftDay === rightDay ? 0 : leftDay < rightDay ? 1 : -1;
    });
}

/**
 * One requested dimension's part of a bucket key, in the operator's words:
 * the day verbatim, ids resolved through the entity index, the enum
 * dimensions through their vocabularies. The part rides the dimension's own
 * `DIMENSION_KEY_FIELD` field — the same pairing the read proved — asserted
 * through `bucketKeyField`, never rendered past when absent. The `NULL`
 * status bucket reads as an em dash — the wire's `null` is a real bucket,
 * never a missing one.
 */
export function usageDimensionLabel(
    dimension: UsageGroupBy,
    key: UsageBucketKey,
    index: UsageEntityIndex,
): string {
    switch (dimension) {
        case "day": {
            const field = DIMENSION_KEY_FIELD[dimension];

            return bucketKeyField(key[field], field);
        }

        case "model": {
            const field = DIMENSION_KEY_FIELD[dimension];

            return usageModelLabel(index.modelsById, bucketKeyField(key[field], field)).text;
        }

        case "provider": {
            const field = DIMENSION_KEY_FIELD[dimension];

            return usageProviderLabel(index.providersById, bucketKeyField(key[field], field)).text;
        }

        case "api_key": {
            const field = DIMENSION_KEY_FIELD[dimension];

            return usageApiKeyLabel(index.apiKeysById, bucketKeyField(key[field], field)).text;
        }

        case "endpoint_type": {
            const field = DIMENSION_KEY_FIELD[dimension];

            return apiLogEndpointLabel(bucketKeyField(key[field], field));
        }

        case "outcome": {
            const field = DIMENSION_KEY_FIELD[dimension];

            return apiLogOutcomeLabel(bucketKeyField(key[field], field));
        }

        case "is_stream": {
            const field = DIMENSION_KEY_FIELD[dimension];

            return bucketKeyField(key[field], field) ? "Streamed" : "Non-streamed";
        }

        case "http_status": {
            const field = DIMENSION_KEY_FIELD[dimension];
            const status = bucketKeyField(key[field], field);

            return status === null ? "—" : String(status);
        }
    }
}
