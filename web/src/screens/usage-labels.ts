import type { ManagementApi } from "/src/management-api/client";
import type {
    ApiKeyResponse,
    ModelResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import { type ReadCall, readAll } from "./read";

/** The entity lists a usage screen reads alongside the accounting rows. */
export type UsageEntities = {
    readonly providers: ReadonlyArray<ProviderResponse>;
    readonly models: ReadonlyArray<ModelResponse>;
    readonly apiKeys: ReadonlyArray<ApiKeyResponse>;
};

/**
 * The three entity listings a usage screen reads alongside its usage rows,
 * as one read call: the listings run in parallel under the cycle's signal
 * and answer as the whole `UsageEntities` triple, so every cycle that ends
 * successfully leaves the labels' resolution lists loaded.
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

export type UsageLabel = {
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
