import type {
    ApiKeyResponse,
    CreateModelRequest,
    ModelResponse,
    ProviderResponse,
    UpdateApiKeyRequest,
    UpdateModelRequest,
    UpdateProviderRequest,
} from "/src/management-api/generated/v0";

/**
 * Read-modify-write edits for `providers:update` (README §4.5): an omitted
 * field keeps the row's value, and the write-only `api_key` keeps the stored
 * secret when absent. Edits never carry the row's own `id`; the request
 * addresses the row through `provider_id`.
 */
export type ProviderEdits = {
    readonly name?: string;
    readonly model_prefix?: string;
    readonly is_enabled?: boolean;
    readonly base_url?: string;
    /** Present replaces the stored static API key; absent keeps it. */
    readonly api_key?: string;
};

/**
 * Builds one `providers:update` request from a listed row and the fields the
 * administrator changed, restating the row's complete desired readable state
 * (README §4.5). OAuth credentials are never carried here: replacing them is
 * the `oauth:reauthorize` flow's job, and omitting the field keeps the stored
 * secret.
 */
export function toUpdateProviderRequest(
    provider: ProviderResponse,
    edits: ProviderEdits = {},
): UpdateProviderRequest {
    switch (provider.type) {
        case "openai_response":
            return {
                type: "openai_response",
                provider_id: provider.id,
                name: edits.name ?? provider.name,
                model_prefix: edits.model_prefix ?? provider.model_prefix,
                is_enabled: edits.is_enabled ?? provider.is_enabled,
                base_url: edits.base_url ?? provider.base_url,
                ...(edits.api_key === undefined
                    ? {}
                    : {
                          api_key: edits.api_key,
                      }),
            };

        case "openai_chat_completion":
            return {
                type: "openai_chat_completion",
                provider_id: provider.id,
                name: edits.name ?? provider.name,
                model_prefix: edits.model_prefix ?? provider.model_prefix,
                is_enabled: edits.is_enabled ?? provider.is_enabled,
                base_url: edits.base_url ?? provider.base_url,
                ...(edits.api_key === undefined
                    ? {}
                    : {
                          api_key: edits.api_key,
                      }),
            };

        case "gemini_generate_content":
            return {
                type: "gemini_generate_content",
                provider_id: provider.id,
                name: edits.name ?? provider.name,
                model_prefix: edits.model_prefix ?? provider.model_prefix,
                is_enabled: edits.is_enabled ?? provider.is_enabled,
                base_url: edits.base_url ?? provider.base_url,
                ...(edits.api_key === undefined
                    ? {}
                    : {
                          api_key: edits.api_key,
                      }),
            };

        case "codex_oauth":
            return {
                type: "codex_oauth",
                provider_id: provider.id,
                name: edits.name ?? provider.name,
                model_prefix: edits.model_prefix ?? provider.model_prefix,
                is_enabled: edits.is_enabled ?? provider.is_enabled,
            };

        case "antigravity_oauth":
            return {
                type: "antigravity_oauth",
                provider_id: provider.id,
                name: edits.name ?? provider.name,
                model_prefix: edits.model_prefix ?? provider.model_prefix,
                is_enabled: edits.is_enabled ?? provider.is_enabled,
            };
    }
}

/**
 * Read-modify-write edits for `models:update` (README §4.5): the row's two
 * writable columns. The nullable mirror column is stated explicitly — `null`
 * is the no-override state — so an edit that clears an override sends `null`
 * rather than omitting the field. `discovery_state` is never an edit: this
 * route cannot restore a vanished row.
 */
export type ModelEdits = {
    readonly id_override?: string | null;
    readonly is_enabled?: boolean;
};

/**
 * Builds one `models:update` request from a listed row and the changed
 * columns. The row is addressed by its never-recycled row identifier; its
 * natural key and its derived public ID are never part of a request.
 */
export function toUpdateModelRequest(
    model: ModelResponse,
    edits: ModelEdits = {},
): UpdateModelRequest {
    return {
        model_id: model.id,
        id_override: edits.id_override === undefined ? model.id_override : edits.id_override,
        is_enabled: edits.is_enabled ?? model.is_enabled,
    };
}

/**
 * Read-modify-write edits for `api-keys:update` (README §4.5): the complete desired state of the
 * row, every field required — a client renders the row, edits it, and sends the same shape back.
 * Unlike provider credentials, the key secret is readable, so it is restated like every other
 * field.
 */
export type ApiKeyEdits = {
    readonly name?: string;
    readonly key?: string;
    readonly is_enabled?: boolean;
};

/**
 * Builds one `api-keys:update` request from a listed row and the changed fields. The row is
 * addressed by `api_key_id`; its server-assigned `id` and `created_at` never travel in a request.
 */
export function toUpdateApiKeyRequest(
    apiKey: ApiKeyResponse,
    edits: ApiKeyEdits = {},
): UpdateApiKeyRequest {
    return {
        api_key_id: apiKey.id,
        name: edits.name ?? apiKey.name,
        key: edits.key ?? apiKey.key,
        is_enabled: edits.is_enabled ?? apiKey.is_enabled,
    };
}

/**
 * Initial state for `models:create`: the two writable columns
 * `models:update` carries — `null` is the no-override state — addressed by a
 * fresh (`provider_id`, `upstream_id`) natural key instead of an existing
 * row. The same shape also claims a vanished row as the manual recovery
 * path.
 */
export type NewModelOverrides = {
    readonly id_override?: string | null;
    readonly is_enabled?: boolean;
};

/**
 * Builds one `models:create` request: the complete desired initial state of a
 * manually added row.
 */
export function toCreateModelRequest(
    providerId: number,
    upstreamId: string,
    overrides: NewModelOverrides = {},
): CreateModelRequest {
    return {
        provider_id: providerId,
        upstream_id: upstreamId,
        id_override: overrides.id_override ?? null,
        is_enabled: overrides.is_enabled ?? true,
    };
}
