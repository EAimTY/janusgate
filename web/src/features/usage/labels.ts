import { translation } from "/src/i18n";
import type {
    ApiKeyResponse,
    ModelResponse,
    ModelRouteResponse,
    ProviderResponse,
    UsageBucket,
    UsageBucketKey,
    UsageGroupBy,
} from "/src/management-api/generated/v0";
import { bucketKeyField, DIMENSION_KEY_FIELD } from "/src/management-api/usage-buckets";
import { API_LOG_ENDPOINT_TYPE, API_LOG_OUTCOME } from "/src/management-api/vocabulary";
import { indexRowsById } from "/src/row-index";
import type { UsageEntities } from "./read";

type UsageLabel = {
    /** The current name, or what kind of row is gone when it is no longer listed. */
    readonly text: string;
    /** False when the id is no longer listed — the row was deleted after the request was recorded. */
    readonly isListed: boolean;
};

/** The usage-domain entity lists indexed by row id, ready for O(1) lookups. */
export type UsageEntityIndex = {
    readonly providersById: ReadonlyMap<number, ProviderResponse>;
    readonly modelRoutesById: ReadonlyMap<number, ModelRouteResponse>;
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
        modelRoutesById: indexRowsById(entities.modelRoutes),
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

/** A request's public route as its current authored name, or a deleted route. */
export function usageModelRouteLabel(
    modelRoutesById: ReadonlyMap<number, ModelRouteResponse>,
    modelRouteId: number,
): UsageLabel {
    return resolveLabel(
        modelRoutesById.get(modelRouteId)?.public_id,
        translation()(($) => $.remainingText.deletedRoute),
    );
}

/** A request's physical model as its upstream ID, or a deleted model. */
export function usageModelLabel(
    modelsById: ReadonlyMap<number, ModelResponse>,
    modelId: number,
): UsageLabel {
    return resolveLabel(
        modelsById.get(modelId)?.upstream_id,
        translation()(($) => $.remainingText.deletedModel),
    );
}

/** A request's provider as its current name, or as a deleted provider. */
export function usageProviderLabel(
    providersById: ReadonlyMap<number, ProviderResponse>,
    providerId: number,
): UsageLabel {
    return resolveLabel(
        providersById.get(providerId)?.name,
        translation()(($) => $.remainingText.deletedProvider),
    );
}

/** A request's API key as its current name, or as a deleted key. */
export function usageApiKeyLabel(
    apiKeysById: ReadonlyMap<number, ApiKeyResponse>,
    apiKeyId: number,
): UsageLabel {
    return resolveLabel(
        apiKeysById.get(apiKeyId)?.name,
        translation()(($) => $.remainingText.deletedKey),
    );
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

        case "model_route": {
            const field = DIMENSION_KEY_FIELD[dimension];
            return usageModelRouteLabel(index.modelRoutesById, bucketKeyField(key[field], field))
                .text;
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

            return API_LOG_ENDPOINT_TYPE.label(bucketKeyField(key[field], field));
        }

        case "outcome": {
            const field = DIMENSION_KEY_FIELD[dimension];

            return API_LOG_OUTCOME.label(bucketKeyField(key[field], field));
        }

        case "is_stream": {
            const field = DIMENSION_KEY_FIELD[dimension];

            return bucketKeyField(key[field], field)
                ? translation()(($) => $.helpers.usageFilterControls.streamed)
                : translation()(($) => $.helpers.usageFilterControls.nonstreamed);
        }

        case "http_status": {
            const field = DIMENSION_KEY_FIELD[dimension];
            const status = bucketKeyField(key[field], field);

            return status === null ? "—" : String(status);
        }
    }
}
