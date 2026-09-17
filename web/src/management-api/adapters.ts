import {
    type ModelResponse,
    type ProviderResponse,
    THINKING_LEVEL_ORDER,
    type ThinkingLevel,
    type UpdateModelRequest,
    type UpdateProviderRequest,
    type UpstreamFormat,
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
    readonly protocol?: UpstreamFormat;
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
        case "openai_compatible":
            return {
                type: "openai_compatible",
                provider_id: provider.id,
                name: edits.name ?? provider.name,
                model_prefix: edits.model_prefix ?? provider.model_prefix,
                is_enabled: edits.is_enabled ?? provider.is_enabled,
                base_url: edits.base_url ?? provider.base_url,
                protocol: edits.protocol ?? provider.protocol,
                ...(edits.api_key === undefined
                    ? {}
                    : {
                          api_key: edits.api_key,
                      }),
            };
        case "openai_oauth":
            return {
                type: "openai_oauth",
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
 * Read-modify-write edits for `models:update` (README §4.5). The two nullable
 * mirror columns are stated explicitly — `null` is the no-override state — so
 * an edit that clears an override sends `null` rather than omitting the field.
 */
export type ModelEdits = {
    readonly id_override?: string | null;
    readonly is_enabled?: boolean;
    readonly thinking_levels_override?: ThinkingLevel[] | null;
};

/**
 * Builds one `models:update` request from a listed row and the changed
 * overrides. The row is addressed by its (`provider_id`, `upstream_id`) pair;
 * its derived public `id` is never part of a request.
 */
export function toUpdateModelRequest(
    model: ModelResponse,
    edits: ModelEdits = {},
): UpdateModelRequest {
    return {
        provider_id: model.provider_id,
        upstream_id: model.upstream_id,
        id_override: edits.id_override === undefined ? model.id_override : edits.id_override,
        is_enabled: edits.is_enabled ?? model.is_enabled,
        thinking_levels_override:
            edits.thinking_levels_override === undefined
                ? model.thinking_levels_override
                : canonicalizeThinkingLevels(edits.thinking_levels_override),
    };
}

/**
 * The canonical order travels with the contract (`THINKING_LEVEL_ORDER`), so a
 * request's override is expressed in that order regardless of the order the
 * editor collected it in.
 */
function canonicalizeThinkingLevels(levels: ThinkingLevel[] | null): ThinkingLevel[] | null {
    if (levels === null) {
        return null;
    }
    return THINKING_LEVEL_ORDER.filter((level) => levels.includes(level));
}
