import type { DefinitionPayload } from "/src/management-api/definition-payload";
import type { DefinitionResponse } from "/src/management-api/generated/v0";

/**
 * The preset catalog (gateway README §5.6): hardcoded, versioned definition documents
 * — one per researched product — that prefill the provider-definition form.
 * The catalog is the only home for presets: the gateway ships no
 * provider-specific anything, and an operator without this frontend authors
 * definition JSON directly against the management API.
 *
 * Every preset carries its reference-implementation provenance and its
 * verification status. `live` means the shape was verified against the real
 * upstream from this codebase; `unverified` means the preset ships on
 * research alone, and flipping it to `live` requires a real upstream run.
 */

/** The verification status one preset carries. */
export type PresetVerification = "live" | "unverified";

/** One preset: a full definition payload plus its presentation facts. */
export type ProviderPreset = {
    /** Stable catalog identifier. */
    readonly id: string;
    /** The product the preset connects. */
    readonly title: string;
    /** One line on what the operator needs beside the preset. */
    readonly description: string;
    /**
     * The grant an OAuth preset runs; `null` for a static-key preset, whose
     * payload credential is an `api_key` — the payload carries the kind.
     */
    readonly grant: "authorization_code" | "device" | null;
    /** Suggested provider name base. */
    readonly suggestedName: string;
    /** The reference-implementation provenance: repo + rev or doc URL. */
    readonly provenance: string;
    /** The verification status: live run or research only. */
    readonly verification: PresetVerification;
    /** The definition payload the form prefills, exactly as the wire carries it. */
    readonly payload: DefinitionPayload;
};

/**
 * The definition JSON the editor prefills for one preset: the preset's
 * payload serialized pretty-printed with a two-space indent.
 */
export function presetDefinitionJson(preset: ProviderPreset): string {
    return JSON.stringify(preset.payload, null, 2);
}

/** Reads one preset's stored definition version, when the gateway holds one. */
export function matchStoredDefinition(
    preset: ProviderPreset,
    definitions: ReadonlyArray<DefinitionResponse>,
): DefinitionResponse | null {
    const stored = definitions.filter((definition) => definition.name === preset.id);
    return stored.length === 0 ? null : (stored.at(-1) ?? null);
}

/** The static-key presets: one per wire format, the generic compatible APIs. */
const STATIC_KEY_PRESETS: readonly [
    ProviderPreset,
    ...ProviderPreset[],
] = [
    {
        id: "generic-openai-responses",
        title: "OpenAI-compatible Responses API",
        description: "Any endpoint speaking the OpenAI Responses protocol with a static key.",
        grant: null,
        suggestedName: "openai",
        provenance:
            "OpenAI Responses API reference (platform.openai.com/docs/api-reference/responses)",
        verification: "live",
        payload: {
            format: "openai_response",
            inference: {
                url: [
                    {
                        type: "base",
                    },
                    {
                        type: "literal",
                        value: "/responses",
                    },
                ],
                upstream_streaming: "match_client",
            },
            credential: {
                type: "api_key",
            },
            discovery: {
                request: {
                    method: "GET",
                    url: [
                        {
                            type: "base",
                        },
                        {
                            type: "literal",
                            value: "/models",
                        },
                    ],
                    headers: [
                        {
                            name: "Authorization",
                            value: [
                                {
                                    type: "literal",
                                    value: "Bearer ",
                                },
                                {
                                    type: "secret",
                                },
                            ],
                        },
                    ],
                },
                extract: {
                    mode: "array",
                    array_at: [
                        "data",
                    ],
                    id_field: [
                        "id",
                    ],
                },
                pagination: {
                    type: "none",
                },
            },
        },
    },
    {
        id: "generic-openai-chat-completions",
        title: "OpenAI-compatible Chat Completions API",
        description: "Any endpoint speaking the Chat Completions protocol with a static key.",
        grant: null,
        suggestedName: "openai",
        provenance:
            "OpenAI Chat Completions API reference (platform.openai.com/docs/api-reference/chat)",
        verification: "live",
        payload: {
            format: "openai_chat_completion",
            inference: {
                url: [
                    {
                        type: "base",
                    },
                    {
                        type: "literal",
                        value: "/chat/completions",
                    },
                ],
                upstream_streaming: "match_client",
            },
            credential: {
                type: "api_key",
            },
            discovery: {
                request: {
                    method: "GET",
                    url: [
                        {
                            type: "base",
                        },
                        {
                            type: "literal",
                            value: "/models",
                        },
                    ],
                    headers: [
                        {
                            name: "Authorization",
                            value: [
                                {
                                    type: "literal",
                                    value: "Bearer ",
                                },
                                {
                                    type: "secret",
                                },
                            ],
                        },
                    ],
                },
                extract: {
                    mode: "array",
                    array_at: [
                        "data",
                    ],
                    id_field: [
                        "id",
                    ],
                },
                pagination: {
                    type: "none",
                },
            },
        },
    },
    {
        id: "generic-gemini-generate-content",
        title: "Gemini-compatible GenerateContent API",
        description: "Any endpoint speaking the Gemini GenerateContent protocol with a static key.",
        grant: null,
        suggestedName: "gemini",
        provenance: "Gemini API reference (ai.google.dev/api/generate-content)",
        verification: "live",
        payload: {
            format: "gemini_generate_content",
            inference: {
                url: [
                    {
                        type: "base",
                    },
                    {
                        type: "model",
                    },
                    {
                        type: "literal",
                        value: ":generateContent",
                    },
                ],
                stream_url: [
                    {
                        type: "base",
                    },
                    {
                        type: "model",
                    },
                    {
                        type: "literal",
                        value: ":streamGenerateContent?alt=sse",
                    },
                ],
                headers: [
                    {
                        name: "x-goog-api-key",
                        value: [
                            {
                                type: "secret",
                            },
                        ],
                    },
                ],
                upstream_streaming: "match_client",
            },
            credential: {
                type: "api_key",
            },
            discovery: {
                request: {
                    method: "GET",
                    url: [
                        {
                            type: "base",
                        },
                        {
                            type: "literal",
                            value: "/models",
                        },
                    ],
                    query: [
                        {
                            name: "pageSize",
                            value: [
                                {
                                    type: "literal",
                                    value: "1000",
                                },
                            ],
                        },
                    ],
                    headers: [
                        {
                            name: "x-goog-api-key",
                            value: [
                                {
                                    type: "secret",
                                },
                            ],
                        },
                    ],
                },
                extract: {
                    mode: "array",
                    array_at: [
                        "models",
                    ],
                    id_field: [
                        "name",
                    ],
                    strip_prefix: "models/",
                },
                pagination: {
                    type: "token",
                    request_param: "pageToken",
                    response_path: [
                        "nextPageToken",
                    ],
                    max_pages: 10,
                },
            },
        },
    },
    {
        id: "generic-gemini-interactions",
        title: "Gemini-compatible Interactions API",
        description: "Any endpoint speaking the Gemini Interactions protocol with a static key.",
        grant: null,
        suggestedName: "gemini",
        provenance: "Gemini Interactions API reference (ai.google.dev/api/interactions-api)",
        verification: "live",
        payload: {
            format: "gemini_interaction",
            inference: {
                url: [
                    {
                        type: "base",
                    },
                    {
                        type: "literal",
                        value: "/v1beta/interactions",
                    },
                ],
                headers: [
                    {
                        name: "x-goog-api-key",
                        value: [
                            {
                                type: "secret",
                            },
                        ],
                    },
                    {
                        name: "Api-Revision",
                        value: [
                            {
                                type: "literal",
                                value: "2026-09-01",
                            },
                        ],
                    },
                ],
                upstream_streaming: "match_client",
            },
            credential: {
                type: "api_key",
            },
            discovery: {
                request: {
                    method: "GET",
                    url: [
                        {
                            type: "base",
                        },
                        {
                            type: "literal",
                            value: "/v1beta/models",
                        },
                    ],
                    query: [
                        {
                            name: "pageSize",
                            value: [
                                {
                                    type: "literal",
                                    value: "1000",
                                },
                            ],
                        },
                    ],
                    headers: [
                        {
                            name: "x-goog-api-key",
                            value: [
                                {
                                    type: "secret",
                                },
                            ],
                        },
                        {
                            name: "Api-Revision",
                            value: [
                                {
                                    type: "literal",
                                    value: "2026-09-01",
                                },
                            ],
                        },
                    ],
                },
                extract: {
                    mode: "array",
                    array_at: [
                        "models",
                    ],
                    id_field: [
                        "name",
                    ],
                    strip_prefix: "models/",
                },
                pagination: {
                    type: "token",
                    request_param: "pageToken",
                    response_path: [
                        "nextPageToken",
                    ],
                    max_pages: 10,
                },
            },
        },
    },
    {
        id: "generic-anthropic-messages",
        title: "Anthropic-compatible Messages API",
        description: "Any endpoint speaking the Anthropic Messages protocol with a static key.",
        grant: null,
        suggestedName: "anthropic",
        provenance: "Anthropic Messages API reference (docs.anthropic.com/en/api/messages)",
        verification: "live",
        payload: {
            format: "anthropic_messages",
            inference: {
                url: [
                    {
                        type: "base",
                    },
                    {
                        type: "literal",
                        value: "/v1/messages",
                    },
                ],
                headers: [
                    {
                        name: "x-api-key",
                        value: [
                            {
                                type: "secret",
                            },
                        ],
                    },
                    {
                        name: "anthropic-version",
                        value: [
                            {
                                type: "literal",
                                value: "2023-06-01",
                            },
                        ],
                    },
                ],
                upstream_streaming: "match_client",
            },
            credential: {
                type: "api_key",
            },
            discovery: {
                request: {
                    method: "GET",
                    url: [
                        {
                            type: "base",
                        },
                        {
                            type: "literal",
                            value: "/v1/models",
                        },
                    ],
                    headers: [
                        {
                            name: "x-api-key",
                            value: [
                                {
                                    type: "secret",
                                },
                            ],
                        },
                        {
                            name: "anthropic-version",
                            value: [
                                {
                                    type: "literal",
                                    value: "2023-06-01",
                                },
                            ],
                        },
                    ],
                },
                extract: {
                    mode: "array",
                    array_at: [
                        "data",
                    ],
                    id_field: [
                        "id",
                    ],
                },
                pagination: {
                    type: "cursor",
                    has_more_path: [
                        "has_more",
                    ],
                    last_id_path: [
                        "last_id",
                    ],
                    request_param: "after_id",
                    max_pages: 10,
                },
            },
        },
    },
    {
        id: "xai",
        title: "xAI (Grok) Responses API",
        description:
            "The xAI Responses endpoint with a static key: native compaction rides the standalone `/responses/compact` operation, while the explicit trigger and the automatic compaction are declared unsupported — a standalone compact endpoint does not imply native trigger or automatic compaction.",
        grant: null,
        suggestedName: "xai",
        provenance:
            "xAI Responses API research (docs.x.ai); compaction facts researched 2026-10-01 — verify against a live compact run before relying on them",
        verification: "unverified",
        payload: {
            format: "openai_response",
            inference: {
                url: [
                    {
                        type: "base",
                    },
                    {
                        type: "literal",
                        value: "/responses",
                    },
                ],
                headers: [
                    {
                        name: "Authorization",
                        value: [
                            {
                                type: "literal",
                                value: "Bearer ",
                            },
                            {
                                type: "secret",
                            },
                        ],
                    },
                ],
                upstream_streaming: "match_client",
            },
            credential: {
                type: "api_key",
            },
            discovery: {
                request: {
                    method: "GET",
                    url: [
                        {
                            type: "base",
                        },
                        {
                            type: "literal",
                            value: "/models",
                        },
                    ],
                    headers: [
                        {
                            name: "Authorization",
                            value: [
                                {
                                    type: "literal",
                                    value: "Bearer ",
                                },
                                {
                                    type: "secret",
                                },
                            ],
                        },
                    ],
                },
                extract: {
                    mode: "array",
                    array_at: [
                        "data",
                    ],
                    id_field: [
                        "id",
                    ],
                },
                pagination: {
                    type: "none",
                },
            },
            compaction: {
                strategy: "native_preferred",
                summary_budget: 4096,
                retention_budget: 8192,
                can_export_native_summary: false,
                mechanisms: {
                    responses_standalone_compact: {
                        default: "supported",
                        operation: {
                            url: [
                                {
                                    type: "base",
                                },
                                {
                                    type: "literal",
                                    value: "/responses/compact",
                                },
                            ],
                            headers: [
                                {
                                    name: "Authorization",
                                    value: [
                                        {
                                            type: "literal",
                                            value: "Bearer ",
                                        },
                                        {
                                            type: "secret",
                                        },
                                    ],
                                },
                            ],
                        },
                    },
                    responses_explicit_trigger: {
                        default: "unsupported",
                    },
                    responses_automatic_compaction: {
                        default: "unsupported",
                    },
                },
            },
        },
    },
];

/** The OAuth presets: the researched products that log in upstream. */
const OAUTH_PRESETS: readonly [
    ProviderPreset,
    ...ProviderPreset[],
] = [
    {
        id: "claude-code",
        title: "Claude Code (Claude account)",
        description:
            "A Claude subscription account over the Messages API: the OAuth token rides `Authorization: Bearer` beside the `oauth-2025-04-20` beta header, on inference and model discovery alike.",
        grant: "authorization_code",
        suggestedName: "claude",
        provenance:
            "anthropics/claude-code research port (hosted paste-back callback, JSON token requests); verify against the live CLI before production use",
        verification: "unverified",
        payload: {
            format: "anthropic_messages",
            inference: {
                url: [
                    {
                        type: "literal",
                        value: "https://api.anthropic.com/v1/messages",
                    },
                ],
                headers: [
                    {
                        name: "Authorization",
                        value: [
                            {
                                type: "literal",
                                value: "Bearer ",
                            },
                            {
                                type: "secret",
                            },
                        ],
                    },
                    {
                        name: "anthropic-beta",
                        value: [
                            {
                                type: "literal",
                                value: "oauth-2025-04-20",
                            },
                        ],
                    },
                    {
                        name: "anthropic-version",
                        value: [
                            {
                                type: "literal",
                                value: "2023-06-01",
                            },
                        ],
                    },
                    {
                        name: "User-Agent",
                        value: [
                            {
                                type: "literal",
                                value: "claude-cli/1.0.0 (external, cli)",
                            },
                        ],
                    },
                ],
                upstream_streaming: "match_client",
            },
            credential: {
                type: "oauth",
                client_id: "9d1c250a-e61b-44e9-88ed-594feddb3c84",
                grant: {
                    type: "authorization_code",
                    authorize_url: [
                        {
                            type: "literal",
                            value: "https://claude.ai/oauth/authorize",
                        },
                    ],
                    redirect_uri: "https://console.anthropic.com/oauth/code/callback",
                    pkce: "s256",
                    extra_params: [
                        {
                            name: "code",
                            value: [
                                {
                                    type: "literal",
                                    value: "true",
                                },
                            ],
                        },
                        {
                            name: "scope",
                            value: [
                                {
                                    type: "literal",
                                    value: "org:create_api_key user:profile user:inference",
                                },
                            ],
                        },
                    ],
                },
                exchange: {
                    request: {
                        method: "POST",
                        url: [
                            {
                                type: "literal",
                                value: "https://console.anthropic.com/v1/oauth/token",
                            },
                        ],
                        headers: [
                            {
                                name: "User-Agent",
                                value: [
                                    {
                                        type: "literal",
                                        value: "claude-cli/1.0.0 (external, cli)",
                                    },
                                ],
                            },
                        ],
                        body: {
                            kind: "json",
                            node: {
                                type: "object",
                                fields: {
                                    grant_type: {
                                        type: "string",
                                        expr: [
                                            {
                                                type: "literal",
                                                value: "authorization_code",
                                            },
                                        ],
                                    },
                                    client_id: {
                                        type: "string",
                                        expr: [
                                            {
                                                type: "client_id",
                                            },
                                        ],
                                    },
                                    code: {
                                        type: "string",
                                        expr: [
                                            {
                                                type: "authorization_code",
                                            },
                                        ],
                                    },
                                    code_verifier: {
                                        type: "string",
                                        expr: [
                                            {
                                                type: "code_verifier",
                                            },
                                        ],
                                    },
                                    redirect_uri: {
                                        type: "string",
                                        expr: [
                                            {
                                                type: "redirect_uri",
                                            },
                                        ],
                                    },
                                },
                            },
                        },
                    },
                    response: {
                        encoding: "json",
                        fields: [
                            {
                                path: [
                                    "access_token",
                                ],
                                target: {
                                    type: "access_token",
                                },
                                required: true,
                            },
                            {
                                path: [
                                    "refresh_token",
                                ],
                                target: {
                                    type: "refresh_token",
                                },
                                required: true,
                            },
                            {
                                path: [
                                    "expires_in",
                                ],
                                target: {
                                    type: "expires_in_seconds",
                                },
                                required: false,
                            },
                        ],
                    },
                    terminal_codes: [
                        "invalid_grant",
                        "expired_token",
                        "invalid_client",
                        "unauthorized_client",
                    ],
                },
                refresh: {
                    type: "grant",
                    request: {
                        method: "POST",
                        url: [
                            {
                                type: "literal",
                                value: "https://console.anthropic.com/v1/oauth/token",
                            },
                        ],
                        headers: [
                            {
                                name: "User-Agent",
                                value: [
                                    {
                                        type: "literal",
                                        value: "claude-cli/1.0.0 (external, cli)",
                                    },
                                ],
                            },
                        ],
                        body: {
                            kind: "json",
                            node: {
                                type: "object",
                                fields: {
                                    grant_type: {
                                        type: "string",
                                        expr: [
                                            {
                                                type: "literal",
                                                value: "refresh_token",
                                            },
                                        ],
                                    },
                                    client_id: {
                                        type: "string",
                                        expr: [
                                            {
                                                type: "client_id",
                                            },
                                        ],
                                    },
                                    refresh_token: {
                                        type: "string",
                                        expr: [
                                            {
                                                type: "refresh_token",
                                            },
                                        ],
                                    },
                                },
                            },
                        },
                    },
                    terminal_codes: [
                        "invalid_grant",
                        "expired_token",
                    ],
                },
            },
            discovery: {
                request: {
                    method: "GET",
                    url: [
                        {
                            type: "literal",
                            value: "https://api.anthropic.com/v1/models",
                        },
                    ],
                    headers: [
                        {
                            name: "Authorization",
                            value: [
                                {
                                    type: "literal",
                                    value: "Bearer ",
                                },
                                {
                                    type: "secret",
                                },
                            ],
                        },
                        {
                            name: "anthropic-beta",
                            value: [
                                {
                                    type: "literal",
                                    value: "oauth-2025-04-20",
                                },
                            ],
                        },
                        {
                            name: "anthropic-version",
                            value: [
                                {
                                    type: "literal",
                                    value: "2023-06-01",
                                },
                            ],
                        },
                    ],
                },
                extract: {
                    mode: "array",
                    array_at: [
                        "data",
                    ],
                    id_field: [
                        "id",
                    ],
                },
                pagination: {
                    type: "cursor",
                    has_more_path: [
                        "has_more",
                    ],
                    last_id_path: [
                        "last_id",
                    ],
                    request_param: "after_id",
                    max_pages: 10,
                },
            },
            compaction: {
                strategy: "native_preferred",
                summary_budget: 4096,
                retention_budget: 8192,
                can_export_native_summary: false,
                mechanisms: {
                    messages_on_demand_compaction: {
                        default: "supported",
                        overlay_headers: [
                            {
                                name: "anthropic-beta",
                                value: [
                                    {
                                        type: "literal",
                                        value: "compact-2026-09-04",
                                    },
                                ],
                            },
                        ],
                    },
                    messages_threshold_compaction: {
                        default: "supported",
                        overlay_headers: [
                            {
                                name: "anthropic-beta",
                                value: [
                                    {
                                        type: "literal",
                                        value: "compact-2026-01-12",
                                    },
                                ],
                            },
                        ],
                    },
                },
            },
        },
    },
    {
        id: "kimi-code",
        title: "Kimi Code (Kimi account)",
        description:
            "A Kimi account over the Coding endpoint: a device-code flow whose device ID slot rides the custom `X-Msh-*` headers on authorization, polling, and refresh alike.",
        grant: "device",
        suggestedName: "kimi",
        provenance:
            "Kimi Code CLI research port (auth.kimi.com device flow, X-Msh-* header set); verify against the live CLI before production use",
        verification: "unverified",
        payload: {
            format: "openai_chat_completion",
            inference: {
                url: [
                    {
                        type: "literal",
                        value: "https://api.kimi.com/coding/v1",
                    },
                    {
                        type: "literal",
                        value: "/chat/completions",
                    },
                ],
                headers: [
                    {
                        name: "Authorization",
                        value: [
                            {
                                type: "literal",
                                value: "Bearer ",
                            },
                            {
                                type: "secret",
                            },
                        ],
                    },
                    {
                        name: "X-Msh-Device-Id",
                        value: [
                            {
                                type: "slot",
                                name: "device_id",
                            },
                        ],
                    },
                    {
                        name: "X-Msh-Platform",
                        value: [
                            {
                                type: "literal",
                                value: "macos",
                            },
                        ],
                    },
                    {
                        name: "User-Agent",
                        value: [
                            {
                                type: "literal",
                                value: "KimiCLI/1.0.0",
                            },
                        ],
                    },
                ],
                upstream_streaming: "match_client",
            },
            credential: {
                type: "oauth",
                client_id: "17e5f67c-37e0-44de-a1c1-33a124c8d3f5",
                initial_slots: [
                    {
                        name: "device_id",
                        generator: "uuid_hex",
                    },
                ],
                grant: {
                    type: "device",
                    authorization: {
                        request: {
                            method: "POST",
                            url: [
                                {
                                    type: "literal",
                                    value: "https://auth.kimi.com/oauth/device_authorization",
                                },
                            ],
                            headers: [
                                {
                                    name: "X-Msh-Device-Id",
                                    value: [
                                        {
                                            type: "slot",
                                            name: "device_id",
                                        },
                                    ],
                                },
                                {
                                    name: "X-Msh-Platform",
                                    value: [
                                        {
                                            type: "literal",
                                            value: "macos",
                                        },
                                    ],
                                },
                                {
                                    name: "User-Agent",
                                    value: [
                                        {
                                            type: "literal",
                                            value: "KimiCLI/1.0.0",
                                        },
                                    ],
                                },
                            ],
                            body: {
                                kind: "form",
                                fields: [
                                    {
                                        name: "client_id",
                                        value: [
                                            {
                                                type: "client_id",
                                            },
                                        ],
                                    },
                                ],
                            },
                        },
                        response: {
                            encoding: "json",
                            fields: [
                                {
                                    path: [
                                        "device_code",
                                    ],
                                    target: {
                                        type: "device_code",
                                    },
                                    required: true,
                                },
                                {
                                    path: [
                                        "user_code",
                                    ],
                                    target: {
                                        type: "user_code",
                                    },
                                    required: true,
                                },
                                {
                                    path: [
                                        "verification_uri_complete",
                                    ],
                                    target: {
                                        type: "verification_uri_complete",
                                    },
                                    required: false,
                                },
                                {
                                    path: [
                                        "verification_uri",
                                    ],
                                    target: {
                                        type: "verification_uri",
                                    },
                                    required: false,
                                },
                                {
                                    path: [
                                        "expires_in",
                                    ],
                                    target: {
                                        type: "expires_in_seconds",
                                    },
                                    required: false,
                                },
                                {
                                    path: [
                                        "interval",
                                    ],
                                    target: {
                                        type: "interval_seconds",
                                    },
                                    required: false,
                                },
                            ],
                        },
                    },
                    expires_in_fallback_seconds: 900,
                },
                exchange: {
                    request: {
                        method: "POST",
                        url: [
                            {
                                type: "literal",
                                value: "https://auth.kimi.com/oauth/token",
                            },
                        ],
                        headers: [
                            {
                                name: "X-Msh-Device-Id",
                                value: [
                                    {
                                        type: "slot",
                                        name: "device_id",
                                    },
                                ],
                            },
                            {
                                name: "X-Msh-Platform",
                                value: [
                                    {
                                        type: "literal",
                                        value: "macos",
                                    },
                                ],
                            },
                            {
                                name: "User-Agent",
                                value: [
                                    {
                                        type: "literal",
                                        value: "KimiCLI/1.0.0",
                                    },
                                ],
                            },
                        ],
                        body: {
                            kind: "form",
                            fields: [
                                {
                                    name: "grant_type",
                                    value: [
                                        {
                                            type: "literal",
                                            value: "urn:ietf:params:oauth:grant-type:device_code",
                                        },
                                    ],
                                },
                                {
                                    name: "client_id",
                                    value: [
                                        {
                                            type: "client_id",
                                        },
                                    ],
                                },
                                {
                                    name: "device_code",
                                    value: [
                                        {
                                            type: "device_code",
                                        },
                                    ],
                                },
                            ],
                        },
                    },
                    response: {
                        encoding: "json",
                        fields: [
                            {
                                path: [
                                    "access_token",
                                ],
                                target: {
                                    type: "access_token",
                                },
                                required: true,
                            },
                            {
                                path: [
                                    "refresh_token",
                                ],
                                target: {
                                    type: "refresh_token",
                                },
                                required: true,
                            },
                            {
                                path: [
                                    "expires_in",
                                ],
                                target: {
                                    type: "expires_in_seconds",
                                },
                                required: false,
                            },
                        ],
                    },
                    terminal_codes: [
                        "expired_token",
                        "access_denied",
                    ],
                },
                refresh: {
                    type: "grant",
                    request: {
                        method: "POST",
                        url: [
                            {
                                type: "literal",
                                value: "https://auth.kimi.com/oauth/token",
                            },
                        ],
                        headers: [
                            {
                                name: "X-Msh-Device-Id",
                                value: [
                                    {
                                        type: "slot",
                                        name: "device_id",
                                    },
                                ],
                            },
                            {
                                name: "X-Msh-Platform",
                                value: [
                                    {
                                        type: "literal",
                                        value: "macos",
                                    },
                                ],
                            },
                            {
                                name: "User-Agent",
                                value: [
                                    {
                                        type: "literal",
                                        value: "KimiCLI/1.0.0",
                                    },
                                ],
                            },
                        ],
                        body: {
                            kind: "form",
                            fields: [
                                {
                                    name: "grant_type",
                                    value: [
                                        {
                                            type: "literal",
                                            value: "refresh_token",
                                        },
                                    ],
                                },
                                {
                                    name: "refresh_token",
                                    value: [
                                        {
                                            type: "refresh_token",
                                        },
                                    ],
                                },
                                {
                                    name: "client_id",
                                    value: [
                                        {
                                            type: "client_id",
                                        },
                                    ],
                                },
                            ],
                        },
                    },
                    terminal_codes: [
                        "invalid_grant",
                    ],
                },
            },
            discovery: {
                request: {
                    method: "GET",
                    url: [
                        {
                            type: "literal",
                            value: "https://api.kimi.com/coding/v1/models",
                        },
                    ],
                    headers: [
                        {
                            name: "Authorization",
                            value: [
                                {
                                    type: "literal",
                                    value: "Bearer ",
                                },
                                {
                                    type: "secret",
                                },
                            ],
                        },
                    ],
                },
                extract: {
                    mode: "array",
                    array_at: [
                        "data",
                    ],
                    id_field: [
                        "id",
                    ],
                },
                pagination: {
                    type: "none",
                },
            },
        },
    },
    {
        id: "github-copilot",
        title: "GitHub Copilot (GitHub account)",
        description:
            "A GitHub account whose durable token is exchanged by reprovisioning for the short-lived Copilot session token and the account's API endpoint.",
        grant: "device",
        suggestedName: "copilot",
        provenance:
            "github/copilot-cli and VS Code Copilot Chat research port (device flow, copilot_internal/v2/token); verify against the live product before production use",
        verification: "unverified",
        payload: {
            format: "openai_chat_completion",
            inference: {
                url: [
                    {
                        type: "base",
                    },
                    {
                        type: "literal",
                        value: "/chat/completions",
                    },
                ],
                headers: [
                    {
                        name: "Authorization",
                        value: [
                            {
                                type: "literal",
                                value: "Bearer ",
                            },
                            {
                                type: "secret",
                            },
                        ],
                    },
                    {
                        name: "Copilot-Integration-Id",
                        value: [
                            {
                                type: "literal",
                                value: "vscode-chat",
                            },
                        ],
                    },
                    {
                        name: "Editor-Version",
                        value: [
                            {
                                type: "literal",
                                value: "vscode/1.99.0",
                            },
                        ],
                    },
                    {
                        name: "User-Agent",
                        value: [
                            {
                                type: "literal",
                                value: "GitHubCopilotChat/1.0.0",
                            },
                        ],
                    },
                ],
                upstream_streaming: "match_client",
            },
            credential: {
                type: "oauth",
                client_id: "Iv1.b507a08c87ecfe98",
                initial_slots: [
                    {
                        name: "device_id",
                        generator: "uuid",
                    },
                ],
                grant: {
                    type: "device",
                    authorization: {
                        request: {
                            method: "POST",
                            url: [
                                {
                                    type: "literal",
                                    value: "https://github.com/login/device/code",
                                },
                            ],
                            headers: [
                                {
                                    name: "Accept",
                                    value: [
                                        {
                                            type: "literal",
                                            value: "application/json",
                                        },
                                    ],
                                },
                            ],
                            body: {
                                kind: "form",
                                fields: [
                                    {
                                        name: "client_id",
                                        value: [
                                            {
                                                type: "client_id",
                                            },
                                        ],
                                    },
                                ],
                            },
                        },
                        response: {
                            encoding: "json",
                            fields: [
                                {
                                    path: [
                                        "device_code",
                                    ],
                                    target: {
                                        type: "device_code",
                                    },
                                    required: true,
                                },
                                {
                                    path: [
                                        "user_code",
                                    ],
                                    target: {
                                        type: "user_code",
                                    },
                                    required: true,
                                },
                                {
                                    path: [
                                        "verification_uri",
                                    ],
                                    target: {
                                        type: "verification_uri",
                                    },
                                    required: false,
                                },
                                {
                                    path: [
                                        "expires_in",
                                    ],
                                    target: {
                                        type: "expires_in_seconds",
                                    },
                                    required: false,
                                },
                                {
                                    path: [
                                        "interval",
                                    ],
                                    target: {
                                        type: "interval_seconds",
                                    },
                                    required: false,
                                },
                            ],
                        },
                    },
                    expires_in_fallback_seconds: 900,
                },
                exchange: {
                    request: {
                        method: "POST",
                        url: [
                            {
                                type: "literal",
                                value: "https://github.com/login/oauth/access_token",
                            },
                        ],
                        headers: [
                            {
                                name: "Accept",
                                value: [
                                    {
                                        type: "literal",
                                        value: "application/json",
                                    },
                                ],
                            },
                        ],
                        body: {
                            kind: "form",
                            fields: [
                                {
                                    name: "grant_type",
                                    value: [
                                        {
                                            type: "literal",
                                            value: "urn:ietf:params:oauth:grant-type:device_code",
                                        },
                                    ],
                                },
                                {
                                    name: "client_id",
                                    value: [
                                        {
                                            type: "client_id",
                                        },
                                    ],
                                },
                                {
                                    name: "device_code",
                                    value: [
                                        {
                                            type: "device_code",
                                        },
                                    ],
                                },
                            ],
                        },
                    },
                    response: {
                        encoding: "json",
                        fields: [
                            {
                                path: [
                                    "access_token",
                                ],
                                target: {
                                    type: "slot",
                                    name: "github_token",
                                },
                                required: true,
                            },
                        ],
                    },
                    terminal_codes: [
                        "expired_token",
                        "access_denied",
                        "unsupported_grant_type",
                        "incorrect_client_credentials",
                        "incorrect_device_code",
                        "device_flow_disabled",
                        "personal_access_token",
                    ],
                },
                refresh: {
                    type: "reprovision",
                },
            },
            provisioning: [
                {
                    request: {
                        method: "POST",
                        url: [
                            {
                                type: "literal",
                                value: "https://api.github.com/copilot_internal/v2/token",
                            },
                        ],
                        headers: [
                            {
                                name: "Authorization",
                                value: [
                                    {
                                        type: "literal",
                                        value: "Bearer ",
                                    },
                                    {
                                        type: "slot",
                                        name: "github_token",
                                    },
                                ],
                            },
                            {
                                name: "Editor-Version",
                                value: [
                                    {
                                        type: "literal",
                                        value: "vscode/1.99.0",
                                    },
                                ],
                            },
                            {
                                name: "Copilot-Integration-Id",
                                value: [
                                    {
                                        type: "literal",
                                        value: "vscode-chat",
                                    },
                                ],
                            },
                        ],
                    },
                    response_encoding: "json",
                    extract: [
                        {
                            path: [
                                "token",
                            ],
                            target: {
                                type: "access_token",
                            },
                            shape: {
                                type: "string",
                            },
                            required: true,
                        },
                        {
                            path: [
                                "expires_at",
                            ],
                            target: {
                                type: "expires_at_unix_seconds",
                            },
                            shape: {
                                type: "integer",
                            },
                            required: true,
                        },
                        {
                            path: [
                                "endpoints",
                                "api",
                            ],
                            target: {
                                type: "base_url",
                            },
                            shape: {
                                type: "string",
                            },
                            required: true,
                        },
                    ],
                },
            ],
            discovery: {
                request: {
                    method: "GET",
                    url: [
                        {
                            type: "base",
                        },
                        {
                            type: "literal",
                            value: "/models",
                        },
                    ],
                    headers: [
                        {
                            name: "Authorization",
                            value: [
                                {
                                    type: "literal",
                                    value: "Bearer ",
                                },
                                {
                                    type: "secret",
                                },
                            ],
                        },
                        {
                            name: "Copilot-Integration-Id",
                            value: [
                                {
                                    type: "literal",
                                    value: "vscode-chat",
                                },
                            ],
                        },
                    ],
                },
                extract: {
                    mode: "array",
                    array_at: [
                        "data",
                    ],
                    id_field: [
                        "id",
                    ],
                },
                pagination: {
                    type: "none",
                },
            },
        },
    },
    {
        id: "devin",
        title: "Devin (Devin account)",
        description:
            "A Devin account whose ConnectRPC token exchange answers a static API key and API server URL — no refresh, no expiry; models are added manually.",
        grant: "authorization_code",
        suggestedName: "devin",
        provenance:
            "Devin OAuth research port (ConnectRPC exchange, api_key/api_server_url fields); the exchange fields and inference protocol carry research-level uncertainty — record a live confirmation before production use",
        verification: "unverified",
        payload: {
            format: "anthropic_messages",
            inference: {
                url: [
                    {
                        type: "base",
                    },
                    {
                        type: "literal",
                        value: "/v1/messages",
                    },
                ],
                headers: [
                    {
                        name: "Authorization",
                        value: [
                            {
                                type: "literal",
                                value: "Bearer ",
                            },
                            {
                                type: "secret",
                            },
                        ],
                    },
                    {
                        name: "anthropic-version",
                        value: [
                            {
                                type: "literal",
                                value: "2023-06-01",
                            },
                        ],
                    },
                ],
                upstream_streaming: "match_client",
            },
            credential: {
                type: "oauth",
                client_id: "devin-oauth-client",
                grant: {
                    type: "authorization_code",
                    authorize_url: [
                        {
                            type: "literal",
                            value: "https://app.devin.ai/oauth/authorize",
                        },
                    ],
                    redirect_uri: "http://localhost:1455/auth/callback",
                    pkce: "s256",
                    extra_params: [],
                },
                exchange: {
                    request: {
                        method: "POST",
                        url: [
                            {
                                type: "literal",
                                value: "https://api.devin.ai/v1/oauth/exchange_token",
                            },
                        ],
                        headers: [
                            {
                                name: "connect-protocol-version",
                                value: [
                                    {
                                        type: "literal",
                                        value: "1",
                                    },
                                ],
                            },
                        ],
                        body: {
                            kind: "json",
                            node: {
                                type: "object",
                                fields: {
                                    grantType: {
                                        type: "string",
                                        expr: [
                                            {
                                                type: "literal",
                                                value: "authorization_code",
                                            },
                                        ],
                                    },
                                    clientId: {
                                        type: "string",
                                        expr: [
                                            {
                                                type: "client_id",
                                            },
                                        ],
                                    },
                                    code: {
                                        type: "string",
                                        expr: [
                                            {
                                                type: "authorization_code",
                                            },
                                        ],
                                    },
                                    codeVerifier: {
                                        type: "string",
                                        expr: [
                                            {
                                                type: "code_verifier",
                                            },
                                        ],
                                    },
                                    redirectUri: {
                                        type: "string",
                                        expr: [
                                            {
                                                type: "redirect_uri",
                                            },
                                        ],
                                    },
                                },
                            },
                        },
                    },
                    response: {
                        encoding: "json",
                        fields: [
                            {
                                path: [
                                    "api_key",
                                ],
                                target: {
                                    type: "access_token",
                                },
                                required: true,
                            },
                            {
                                path: [
                                    "api_server_url",
                                ],
                                target: {
                                    type: "base_url",
                                },
                                required: true,
                            },
                        ],
                    },
                    terminal_codes: [
                        "invalid_grant",
                        "unauthenticated",
                    ],
                },
                refresh: {
                    type: "none",
                },
            },
        },
    },

    {
        id: "codex",
        title: "Codex (ChatGPT account)",
        description:
            "A ChatGPT account over the Codex Responses endpoint. Streaming is forced upstream and responses render with the OpenAI Responses format. The compaction section declares the explicit trigger and the automatic threshold compaction the Codex backend accepts.",
        grant: "authorization_code",
        suggestedName: "codex",
        provenance:
            "openai/codex at rev fcf05456bb27e6c3d5677550f54011db6a2a0817 (codex-rs/login; login and inference verified against the live upstream); the compaction declarations are researched from the Codex client and backend behavior at rev 444da310 and remain unverified against a live compact run",
        verification: "unverified",
        payload: {
            format: "openai_response",
            inference: {
                url: [
                    {
                        type: "literal",
                        value: "https://chatgpt.com/backend-api/codex/responses",
                    },
                ],
                headers: [
                    {
                        name: "Authorization",
                        value: [
                            {
                                type: "literal",
                                value: "Bearer ",
                            },
                            {
                                type: "secret",
                            },
                        ],
                    },
                    {
                        name: "chatgpt-account-id",
                        value: [
                            {
                                type: "slot",
                                name: "account_id",
                            },
                        ],
                    },
                    {
                        name: "Originator",
                        value: [
                            {
                                type: "literal",
                                value: "codex_cli_rs",
                            },
                        ],
                    },
                    {
                        name: "User-Agent",
                        value: [
                            {
                                type: "literal",
                                value: "codex_cli_rs/0.147.0",
                            },
                        ],
                    },
                    {
                        name: "Session-Id",
                        value: [
                            {
                                type: "request_uuid",
                            },
                        ],
                    },
                ],
                upstream_streaming: "always",
                render_options: [
                    "omit_max_output_tokens",
                ],
            },
            credential: {
                type: "oauth",
                client_id: "app_EMoamEEZ73f0CkXaXp7hrann",
                grant: {
                    type: "authorization_code",
                    authorize_url: [
                        {
                            type: "literal",
                            value: "https://auth.openai.com/oauth/authorize",
                        },
                    ],
                    redirect_uri: "http://localhost:1455/auth/callback",
                    pkce: "s256",
                    extra_params: [
                        {
                            name: "scope",
                            value: [
                                {
                                    type: "literal",
                                    value: "openid profile email offline_access api.connectors.read api.connectors.invoke",
                                },
                            ],
                        },
                        {
                            name: "id_token_add_organizations",
                            value: [
                                {
                                    type: "literal",
                                    value: "true",
                                },
                            ],
                        },
                        {
                            name: "codex_cli_simplified_flow",
                            value: [
                                {
                                    type: "literal",
                                    value: "true",
                                },
                            ],
                        },
                        {
                            name: "originator",
                            value: [
                                {
                                    type: "literal",
                                    value: "codex_cli_rs",
                                },
                            ],
                        },
                    ],
                },
                exchange: {
                    request: {
                        method: "POST",
                        url: [
                            {
                                type: "literal",
                                value: "https://auth.openai.com/oauth/token",
                            },
                        ],
                        body: {
                            kind: "form",
                            fields: [
                                {
                                    name: "grant_type",
                                    value: [
                                        {
                                            type: "literal",
                                            value: "authorization_code",
                                        },
                                    ],
                                },
                                {
                                    name: "code",
                                    value: [
                                        {
                                            type: "authorization_code",
                                        },
                                    ],
                                },
                                {
                                    name: "redirect_uri",
                                    value: [
                                        {
                                            type: "redirect_uri",
                                        },
                                    ],
                                },
                                {
                                    name: "client_id",
                                    value: [
                                        {
                                            type: "client_id",
                                        },
                                    ],
                                },
                                {
                                    name: "code_verifier",
                                    value: [
                                        {
                                            type: "code_verifier",
                                        },
                                    ],
                                },
                            ],
                        },
                    },
                    response: {
                        encoding: "json",
                        fields: [
                            {
                                path: [
                                    "access_token",
                                ],
                                target: {
                                    type: "access_token",
                                },
                                required: true,
                            },
                            {
                                path: [
                                    "refresh_token",
                                ],
                                target: {
                                    type: "refresh_token",
                                },
                                required: true,
                            },
                            {
                                path: [
                                    "id_token",
                                ],
                                target: {
                                    type: "id_token",
                                },
                                required: true,
                            },
                            {
                                path: [
                                    "expires_in",
                                ],
                                target: {
                                    type: "expires_in_seconds",
                                },
                                required: false,
                            },
                        ],
                    },
                    terminal_codes: [
                        "invalid_grant",
                        "token_expired",
                        "invalid_token",
                        "token_exchange_user_error",
                    ],
                },
                id_token_claims: [
                    {
                        claim: [
                            "https://api.openai.com/auth",
                            "chatgpt_account_id",
                        ],
                        slot: "account_id",
                    },
                ],
                refresh: {
                    type: "grant",
                    request: {
                        method: "POST",
                        url: [
                            {
                                type: "literal",
                                value: "https://auth.openai.com/oauth/token",
                            },
                        ],
                        headers: [
                            {
                                name: "User-Agent",
                                value: [
                                    {
                                        type: "literal",
                                        value: "codex_cli_rs/0.147.0",
                                    },
                                ],
                            },
                        ],
                        body: {
                            kind: "json",
                            node: {
                                type: "object",
                                fields: {
                                    client_id: {
                                        type: "string",
                                        expr: [
                                            {
                                                type: "client_id",
                                            },
                                        ],
                                    },
                                    grant_type: {
                                        type: "string",
                                        expr: [
                                            {
                                                type: "literal",
                                                value: "refresh_token",
                                            },
                                        ],
                                    },
                                    refresh_token: {
                                        type: "string",
                                        expr: [
                                            {
                                                type: "refresh_token",
                                            },
                                        ],
                                    },
                                },
                            },
                        },
                    },
                    terminal_codes: [
                        "invalid_grant",
                        "token_expired",
                        "invalid_token",
                        "refresh_token_expired",
                        "refresh_token_reused",
                        "refresh_token_invalidated",
                    ],
                },
            },
            discovery: {
                request: {
                    method: "GET",
                    url: [
                        {
                            type: "literal",
                            value: "https://chatgpt.com/backend-api/codex/models",
                        },
                    ],
                    query: [
                        {
                            name: "client_version",
                            value: [
                                {
                                    type: "literal",
                                    value: "0.147.0",
                                },
                            ],
                        },
                    ],
                    headers: [
                        {
                            name: "Authorization",
                            value: [
                                {
                                    type: "literal",
                                    value: "Bearer ",
                                },
                                {
                                    type: "secret",
                                },
                            ],
                        },
                        {
                            name: "chatgpt-account-id",
                            value: [
                                {
                                    type: "slot",
                                    name: "account_id",
                                },
                            ],
                        },
                    ],
                },
                extract: {
                    mode: "array",
                    array_at: [
                        "items",
                    ],
                    id_field: [
                        "slug",
                    ],
                },
                pagination: {
                    type: "none",
                },
            },
            compaction: {
                strategy: "native_preferred",
                summary_budget: 4096,
                retention_budget: 8192,
                can_export_native_summary: false,
                mechanisms: {
                    responses_explicit_trigger: {
                        default: "supported",
                    },
                    responses_automatic_compaction: {
                        default: "supported",
                    },
                },
            },
        },
    },
    {
        id: "antigravity",
        title: "Antigravity (Google account)",
        description:
            "A Google account over the Antigravity Gemini endpoint, with project onboarding and an enveloped GenerateContent request.",
        grant: "authorization_code",
        suggestedName: "antigravity",
        provenance:
            "NoeFabris/opencode-antigravity-auth at rev 16e0056 (src/antigravity/oauth.ts, src/plugin)",
        verification: "live",
        payload: {
            format: "gemini_generate_content",
            inference: {
                url: [
                    {
                        type: "literal",
                        value: "https://daily-cloudcode-pa.googleapis.com/v1internal:generateContent",
                    },
                ],
                stream_url: [
                    {
                        type: "literal",
                        value: "https://daily-cloudcode-pa.googleapis.com/v1internal:streamGenerateContent?alt=sse",
                    },
                ],
                headers: [
                    {
                        name: "Authorization",
                        value: [
                            {
                                type: "literal",
                                value: "Bearer ",
                            },
                            {
                                type: "secret",
                            },
                        ],
                    },
                    {
                        name: "User-Agent",
                        value: [
                            {
                                type: "literal",
                                value: "antigravity",
                            },
                        ],
                    },
                ],
                upstream_streaming: "match_client",
                envelope: {
                    type: "object",
                    fields: {
                        project: {
                            type: "string",
                            expr: [
                                {
                                    type: "slot",
                                    name: "project",
                                },
                            ],
                        },
                        model: {
                            type: "string",
                            expr: [
                                {
                                    type: "model",
                                },
                            ],
                        },
                        request: {
                            type: "body",
                        },
                        requestType: {
                            type: "json",
                            value: "agent",
                        },
                        userAgent: {
                            type: "json",
                            value: "antigravity",
                        },
                        requestId: {
                            type: "string",
                            expr: [
                                {
                                    type: "literal",
                                    value: "agent-",
                                },
                                {
                                    type: "request_uuid",
                                },
                            ],
                        },
                    },
                },
                response_unwrap: "response",
            },
            credential: {
                type: "oauth",
                client_id:
                    "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com",
                client_secret: "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",
                initial_slots: [
                    {
                        name: "device_id",
                        generator: "uuid_hex",
                    },
                ],
                grant: {
                    type: "authorization_code",
                    authorize_url: [
                        {
                            type: "literal",
                            value: "https://accounts.google.com/o/oauth2/v2/auth",
                        },
                    ],
                    redirect_uri: "http://localhost:51121/oauth-callback",
                    pkce: "none",
                    extra_params: [
                        {
                            name: "access_type",
                            value: [
                                {
                                    type: "literal",
                                    value: "offline",
                                },
                            ],
                        },
                        {
                            name: "prompt",
                            value: [
                                {
                                    type: "literal",
                                    value: "consent",
                                },
                            ],
                        },
                        {
                            name: "scope",
                            value: [
                                {
                                    type: "literal",
                                    value: "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/userinfo.profile https://www.googleapis.com/auth/cclog https://www.googleapis.com/auth/experimentsandconfigs",
                                },
                            ],
                        },
                    ],
                },
                exchange: {
                    request: {
                        method: "POST",
                        url: [
                            {
                                type: "literal",
                                value: "https://oauth2.googleapis.com/token",
                            },
                        ],
                        body: {
                            kind: "form",
                            fields: [
                                {
                                    name: "grant_type",
                                    value: [
                                        {
                                            type: "literal",
                                            value: "authorization_code",
                                        },
                                    ],
                                },
                                {
                                    name: "code",
                                    value: [
                                        {
                                            type: "authorization_code",
                                        },
                                    ],
                                },
                                {
                                    name: "client_id",
                                    value: [
                                        {
                                            type: "client_id",
                                        },
                                    ],
                                },
                                {
                                    name: "client_secret",
                                    value: [
                                        {
                                            type: "client_secret",
                                        },
                                    ],
                                },
                                {
                                    name: "redirect_uri",
                                    value: [
                                        {
                                            type: "redirect_uri",
                                        },
                                    ],
                                },
                            ],
                        },
                    },
                    response: {
                        encoding: "json",
                        fields: [
                            {
                                path: [
                                    "access_token",
                                ],
                                target: {
                                    type: "access_token",
                                },
                                required: true,
                            },
                            {
                                path: [
                                    "refresh_token",
                                ],
                                target: {
                                    type: "refresh_token",
                                },
                                required: true,
                            },
                            {
                                path: [
                                    "expires_in",
                                ],
                                target: {
                                    type: "expires_in_seconds",
                                },
                                required: false,
                            },
                        ],
                    },
                    terminal_codes: [
                        "invalid_grant",
                    ],
                },
                refresh: {
                    type: "grant",
                    request: {
                        method: "POST",
                        url: [
                            {
                                type: "literal",
                                value: "https://oauth2.googleapis.com/token",
                            },
                        ],
                        body: {
                            kind: "form",
                            fields: [
                                {
                                    name: "grant_type",
                                    value: [
                                        {
                                            type: "literal",
                                            value: "refresh_token",
                                        },
                                    ],
                                },
                                {
                                    name: "refresh_token",
                                    value: [
                                        {
                                            type: "refresh_token",
                                        },
                                    ],
                                },
                                {
                                    name: "client_id",
                                    value: [
                                        {
                                            type: "client_id",
                                        },
                                    ],
                                },
                                {
                                    name: "client_secret",
                                    value: [
                                        {
                                            type: "client_secret",
                                        },
                                    ],
                                },
                            ],
                        },
                    },
                    terminal_codes: [
                        "invalid_grant",
                    ],
                },
            },
            provisioning: [
                {
                    request: {
                        method: "POST",
                        url: [
                            {
                                type: "literal",
                                value: "https://daily-cloudcode-pa.googleapis.com/v1internal:loadCodeAssist",
                            },
                        ],
                        headers: [
                            {
                                name: "Authorization",
                                value: [
                                    {
                                        type: "literal",
                                        value: "Bearer ",
                                    },
                                    {
                                        type: "secret",
                                    },
                                ],
                            },
                            {
                                name: "User-Agent",
                                value: [
                                    {
                                        type: "literal",
                                        value: "antigravity",
                                    },
                                ],
                            },
                        ],
                        body: {
                            kind: "json",
                            node: {
                                type: "object",
                                fields: {
                                    metadata: {
                                        type: "json",
                                        value: {
                                            ideType: "ANTIGRAVITY",
                                            platform: "PLATFORM_UNSPECIFIED",
                                            pluginType: "GEMINI",
                                        },
                                    },
                                },
                            },
                        },
                    },
                    response_encoding: "json",
                    extract: [
                        {
                            path: [
                                "cloudaicompanionProject",
                            ],
                            target: {
                                type: "slot",
                                name: "project",
                            },
                            shape: {
                                type: "string_or_field",
                                field: "id",
                            },
                            required: false,
                        },
                        {
                            path: [
                                "paidTier",
                                "id",
                            ],
                            target: {
                                type: "slot",
                                name: "paid_tier",
                            },
                            shape: {
                                type: "string",
                            },
                            required: false,
                        },
                        {
                            path: [
                                "currentTier",
                                "id",
                            ],
                            target: {
                                type: "slot",
                                name: "current_tier",
                            },
                            shape: {
                                type: "string",
                            },
                            required: false,
                        },
                    ],
                    skip_if: "project",
                    poll: {
                        request: {
                            method: "POST",
                            url: [
                                {
                                    type: "literal",
                                    value: "https://daily-cloudcode-pa.googleapis.com/v1internal:onboardUser",
                                },
                            ],
                            headers: [
                                {
                                    name: "Authorization",
                                    value: [
                                        {
                                            type: "literal",
                                            value: "Bearer ",
                                        },
                                        {
                                            type: "secret",
                                        },
                                    ],
                                },
                                {
                                    name: "User-Agent",
                                    value: [
                                        {
                                            type: "literal",
                                            value: "antigravity",
                                        },
                                    ],
                                },
                            ],
                            body: {
                                kind: "json",
                                node: {
                                    type: "object",
                                    fields: {
                                        tierId: {
                                            type: "string",
                                            expr: [
                                                {
                                                    type: "coalesce",
                                                    parts: [
                                                        {
                                                            type: "slot",
                                                            name: "paid_tier",
                                                        },
                                                        {
                                                            type: "slot",
                                                            name: "current_tier",
                                                        },
                                                        {
                                                            type: "literal",
                                                            value: "free-tier",
                                                        },
                                                    ],
                                                },
                                            ],
                                        },
                                        metadata: {
                                            type: "json",
                                            value: {
                                                ideType: "ANTIGRAVITY",
                                                platform: "PLATFORM_UNSPECIFIED",
                                                pluginType: "GEMINI",
                                            },
                                        },
                                    },
                                },
                            },
                        },
                        response_encoding: "json",
                        done_path: [
                            "done",
                        ],
                        error_path: [
                            "error",
                        ],
                        result_extract: [
                            {
                                path: [
                                    "response",
                                    "cloudaicompanionProject",
                                ],
                                target: {
                                    type: "slot",
                                    name: "project",
                                },
                                shape: {
                                    type: "string_or_field",
                                    field: "id",
                                },
                                required: true,
                            },
                        ],
                        interval_ms: 1000,
                        budget_ms: 60000,
                    },
                },
            ],
            discovery: {
                request: {
                    method: "POST",
                    url: [
                        {
                            type: "literal",
                            value: "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels",
                        },
                    ],
                    headers: [
                        {
                            name: "Authorization",
                            value: [
                                {
                                    type: "literal",
                                    value: "Bearer ",
                                },
                                {
                                    type: "secret",
                                },
                            ],
                        },
                        {
                            name: "User-Agent",
                            value: [
                                {
                                    type: "literal",
                                    value: "antigravity",
                                },
                            ],
                        },
                    ],
                    body: {
                        kind: "json",
                        node: {
                            type: "object",
                            fields: {},
                        },
                    },
                },
                extract: {
                    mode: "map_keys",
                    at: [
                        "models",
                    ],
                },
                pagination: {
                    type: "none",
                },
            },
        },
    },
];

/**
 * The OpenCode Zen presets: one static-key definition per wire format, each
 * against the same documented base and the same ID-list model discovery. The
 * lists carry identifiers regardless of per-model endpoint support — an
 * unsupported combination fails upstream, and the operator disables the row
 * or adds models manually (README §5.6).
 */
const ZEN_PRESETS: readonly [
    ProviderPreset,
    ...ProviderPreset[],
] = [
    {
        id: "zen-openai-chat-completions",
        title: "OpenCode Zen — Chat Completions",
        description: "Zen over the OpenAI Chat Completions protocol with a static key.",
        grant: null,
        suggestedName: "zen",
        provenance: "OpenCode Zen documentation (opencode.ai/docs/zen)",
        verification: "unverified",
        payload: {
            format: "openai_chat_completion",
            inference: {
                url: [
                    {
                        type: "literal",
                        value: "https://opencode.ai/zen/v1",
                    },
                    {
                        type: "literal",
                        value: "/chat/completions",
                    },
                ],
                headers: [
                    {
                        name: "Authorization",
                        value: [
                            {
                                type: "literal",
                                value: "Bearer ",
                            },
                            {
                                type: "secret",
                            },
                        ],
                    },
                ],
                upstream_streaming: "match_client",
            },
            credential: {
                type: "api_key",
            },
            discovery: {
                request: {
                    method: "GET",
                    url: [
                        {
                            type: "literal",
                            value: "https://opencode.ai/zen/v1/models",
                        },
                    ],
                    headers: [
                        {
                            name: "Authorization",
                            value: [
                                {
                                    type: "literal",
                                    value: "Bearer ",
                                },
                                {
                                    type: "secret",
                                },
                            ],
                        },
                    ],
                },
                extract: {
                    mode: "array",
                    array_at: [
                        "data",
                    ],
                    id_field: [
                        "id",
                    ],
                },
                pagination: {
                    type: "none",
                },
            },
        },
    },
    {
        id: "zen-openai-responses",
        title: "OpenCode Zen — Responses",
        description: "Zen over the OpenAI Responses protocol with a static key.",
        grant: null,
        suggestedName: "zen",
        provenance: "OpenCode Zen documentation (opencode.ai/docs/zen)",
        verification: "unverified",
        payload: {
            format: "openai_response",
            inference: {
                url: [
                    {
                        type: "literal",
                        value: "https://opencode.ai/zen/v1",
                    },
                    {
                        type: "literal",
                        value: "/responses",
                    },
                ],
                headers: [
                    {
                        name: "Authorization",
                        value: [
                            {
                                type: "literal",
                                value: "Bearer ",
                            },
                            {
                                type: "secret",
                            },
                        ],
                    },
                ],
                upstream_streaming: "match_client",
            },
            credential: {
                type: "api_key",
            },
            discovery: {
                request: {
                    method: "GET",
                    url: [
                        {
                            type: "literal",
                            value: "https://opencode.ai/zen/v1/models",
                        },
                    ],
                    headers: [
                        {
                            name: "Authorization",
                            value: [
                                {
                                    type: "literal",
                                    value: "Bearer ",
                                },
                                {
                                    type: "secret",
                                },
                            ],
                        },
                    ],
                },
                extract: {
                    mode: "array",
                    array_at: [
                        "data",
                    ],
                    id_field: [
                        "id",
                    ],
                },
                pagination: {
                    type: "none",
                },
            },
        },
    },
    {
        id: "zen-anthropic-messages",
        title: "OpenCode Zen — Anthropic Messages",
        description: "Zen over the Anthropic Messages protocol with a static key.",
        grant: null,
        suggestedName: "zen",
        provenance: "OpenCode Zen documentation (opencode.ai/docs/zen)",
        verification: "unverified",
        payload: {
            format: "anthropic_messages",
            inference: {
                url: [
                    {
                        type: "literal",
                        value: "https://opencode.ai/zen/v1",
                    },
                    {
                        type: "literal",
                        value: "/v1/messages",
                    },
                ],
                headers: [
                    {
                        name: "x-api-key",
                        value: [
                            {
                                type: "secret",
                            },
                        ],
                    },
                    {
                        name: "anthropic-version",
                        value: [
                            {
                                type: "literal",
                                value: "2023-06-01",
                            },
                        ],
                    },
                ],
                upstream_streaming: "match_client",
            },
            credential: {
                type: "api_key",
            },
            discovery: {
                request: {
                    method: "GET",
                    url: [
                        {
                            type: "literal",
                            value: "https://opencode.ai/zen/v1/models",
                        },
                    ],
                    headers: [
                        {
                            name: "x-api-key",
                            value: [
                                {
                                    type: "secret",
                                },
                            ],
                        },
                    ],
                },
                extract: {
                    mode: "array",
                    array_at: [
                        "data",
                    ],
                    id_field: [
                        "id",
                    ],
                },
                pagination: {
                    type: "none",
                },
            },
        },
    },
    {
        id: "zen-gemini-generate-content",
        title: "OpenCode Zen — Gemini GenerateContent",
        description: "Zen over the Gemini GenerateContent protocol with a static key.",
        grant: null,
        suggestedName: "zen",
        provenance: "OpenCode Zen documentation (opencode.ai/docs/zen)",
        verification: "unverified",
        payload: {
            format: "gemini_generate_content",
            inference: {
                url: [
                    {
                        type: "literal",
                        value: "https://opencode.ai/zen/v1",
                    },
                    {
                        type: "model",
                    },
                    {
                        type: "literal",
                        value: ":generateContent",
                    },
                ],
                stream_url: [
                    {
                        type: "literal",
                        value: "https://opencode.ai/zen/v1",
                    },
                    {
                        type: "model",
                    },
                    {
                        type: "literal",
                        value: ":streamGenerateContent?alt=sse",
                    },
                ],
                headers: [
                    {
                        name: "x-goog-api-key",
                        value: [
                            {
                                type: "secret",
                            },
                        ],
                    },
                ],
                upstream_streaming: "match_client",
            },
            credential: {
                type: "api_key",
            },
            discovery: {
                request: {
                    method: "GET",
                    url: [
                        {
                            type: "literal",
                            value: "https://opencode.ai/zen/v1/models",
                        },
                    ],
                    headers: [
                        {
                            name: "x-goog-api-key",
                            value: [
                                {
                                    type: "secret",
                                },
                            ],
                        },
                    ],
                },
                extract: {
                    mode: "array",
                    array_at: [
                        "data",
                    ],
                    id_field: [
                        "id",
                    ],
                },
                pagination: {
                    type: "none",
                },
            },
        },
    },
];

/**
 * Every catalog entry, research presets included. The tuple is never empty —
 * the picker's default selection and the fallback preset read the first
 * entry without a check.
 */
export const PROVIDER_PRESETS: readonly [
    ProviderPreset,
    ...ProviderPreset[],
] = [
    ...STATIC_KEY_PRESETS,
    ...OAUTH_PRESETS,
    ...ZEN_PRESETS,
];
