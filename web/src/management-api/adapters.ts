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
 * Read-modify-write edits for `providers:update` (gateway README §7.1): an omitted
 * field keeps the row's value, and the write-only `api_key` keeps the stored
 * secret when absent. Edits never carry the row's own `id`; the request
 * addresses the row through `provider_id`.
 */
type ProviderEdits = {
    readonly name?: string;
    readonly model_prefix?: string;
    readonly is_enabled?: boolean;
    /** A present `null` is the wire value to send — legal when the effective definition does not reference the base — never a fall-back to the stored one. */
    readonly base_url?: string | null;
    /** Present replaces the stored static API key; absent keeps it. */
    readonly api_key?: string;
};

/**
 * Builds one `providers:update` request from a listed row and the fields the
 * administrator changed, restating the row's complete desired readable state
 * (README §6.1). Every row shares one wire shape — the definition pair, the
 * derived format, and the nullable base URL — so there is one branch. OAuth
 * credentials are never carried here: replacing them is an OAuth flow's job,
 * and omitting `api_key` keeps the stored secret.
 */
export function toUpdateProviderRequest(
    provider: ProviderResponse,
    edits: ProviderEdits = {},
): UpdateProviderRequest {
    return {
        provider_id: provider.id,
        name: edits.name ?? provider.name,
        model_prefix: edits.model_prefix ?? provider.model_prefix,
        is_enabled: edits.is_enabled ?? provider.is_enabled,
        base_url: edits.base_url === undefined ? provider.base_url : edits.base_url,
        ...(edits.api_key === undefined
            ? {}
            : {
                  api_key: edits.api_key,
              }),
    };
}

/**
 * Optional overrides of the model row's five writable columns (README
 * §4.5). The nullable mirror columns are stated explicitly — `null` is
 * the no-override state — so an override that clears one sends `null`
 * rather than omitting the field. `models:create` applies the overrides
 * over the defaults, `models:update` over the listed row.
 * `discovery_state` and the capability base columns are never among them:
 * neither route can restore a vanished row, and no write states discovery
 * knowledge.
 */
type ModelOverrides = {
    readonly id_override?: string | null;
    readonly is_enabled?: boolean;
    /**
     * The thinking-level override; `null` keeps the discovered value, an
     * array replaces the declaration wholesale — the empty array is the
     * positive declaration of no levels.
     */
    readonly thinking_levels_override?: Array<string> | null;
    /** The maximum-input-tokens override; `null` keeps the discovered value. */
    readonly max_input_tokens_override?: number | null;
    /** The maximum-output-tokens override; `null` keeps the discovered value. */
    readonly max_output_tokens_override?: number | null;
};

/**
 * Builds one `models:update` request from a listed row and the changed
 * columns. The row is addressed by its never-recycled row identifier; its
 * natural key and its derived public ID are never part of a request.
 */
export function toUpdateModelRequest(
    model: ModelResponse,
    edits: ModelOverrides = {},
): UpdateModelRequest {
    return {
        model_id: model.id,
        id_override: edits.id_override === undefined ? model.id_override : edits.id_override,
        is_enabled: edits.is_enabled ?? model.is_enabled,
        thinking_levels_override:
            edits.thinking_levels_override === undefined
                ? model.thinking_levels_override
                : edits.thinking_levels_override,
        max_input_tokens_override:
            edits.max_input_tokens_override === undefined
                ? model.max_input_tokens_override
                : edits.max_input_tokens_override,
        max_output_tokens_override:
            edits.max_output_tokens_override === undefined
                ? model.max_output_tokens_override
                : edits.max_output_tokens_override,
    };
}

/**
 * Read-modify-write edits for `api-keys:update` (README §4.5): the complete desired state of the
 * row, every field required — a client renders the row, edits it, and sends the same shape back.
 * Unlike provider credentials, the key secret is readable, so it is restated like every other
 * field.
 */
type ApiKeyEdits = {
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
 * Builds one `models:create` request: the complete desired initial state of a
 * manually added row.
 */
export function toCreateModelRequest(
    providerId: number,
    upstreamId: string,
    overrides: ModelOverrides = {},
): CreateModelRequest {
    return {
        provider_id: providerId,
        upstream_id: upstreamId,
        id_override: overrides.id_override ?? null,
        is_enabled: overrides.is_enabled ?? true,
        thinking_levels_override: overrides.thinking_levels_override ?? null,
        max_input_tokens_override: overrides.max_input_tokens_override ?? null,
        max_output_tokens_override: overrides.max_output_tokens_override ?? null,
    };
}
