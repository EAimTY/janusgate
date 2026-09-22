import type {
    ApiKeyResponse,
    ModelResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";

/** The entity lists a usage screen reads alongside the accounting rows. */
export type UsageEntities = {
    readonly providers: ReadonlyArray<ProviderResponse>;
    readonly models: ReadonlyArray<ModelResponse>;
    readonly apiKeys: ReadonlyArray<ApiKeyResponse>;
};

export type UsageLabel = {
    /** The current name, or the bare `#<id>` when the row is gone. */
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
): UsageLabel {
    const row = names.find((candidate) => candidate.id === id);

    return row === undefined
        ? {
              text: `#${id}`,
              isListed: false,
          }
        : {
              text: row.name,
              isListed: true,
          };
}

/** A request's model as its current public ID, or its bare number. */
export function usageModelLabel(entities: UsageEntities, modelId: number): UsageLabel {
    return resolveLabel(
        entities.models.map((row) => ({
            id: row.id,
            name: row.public_id,
        })),
        modelId,
    );
}

/** A request's provider as its current name, or its bare number. */
export function usageProviderLabel(entities: UsageEntities, providerId: number): UsageLabel {
    return resolveLabel(entities.providers, providerId);
}

/** A request's API key as its current name, or its bare number. */
export function usageApiKeyLabel(entities: UsageEntities, apiKeyId: number): UsageLabel {
    return resolveLabel(entities.apiKeys, apiKeyId);
}
