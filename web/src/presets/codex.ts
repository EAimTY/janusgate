import { translation } from "../i18n";
import type { QuotaEntryExtraction } from "../management-api/generated/v0";
import { bearer, field, numberField, ratioEntry } from "./quota-helpers";
import type { ProviderPreset } from "./types";

/** Codex uses upstream durations; window names do not imply five hours or seven days. */
function codexWindows(at: string[] = []): QuotaEntryExtraction[] {
    return [
        "primary",
        "secondary",
    ].map((key) =>
        ratioEntry(
            key,
            key === "primary"
                ? translation()(($) => $.helpers.presetQuotas.primaryWindow)
                : translation()(($) => $.helpers.presetQuotas.secondaryWindow),
            [
                ...at,
                `${key}_window`,
            ],
            [
                "used_percent",
            ],
            {
                type: "duration_field",
                value: numberField([
                    "limit_window_seconds",
                ]),
                unit: "seconds",
            },
            [
                "reset_at",
            ],
            "unix_seconds",
        ),
    );
}

/** Complete authored preset for codex. */
export const codexPreset: ProviderPreset = {
    id: "codex",
    get title() {
        return translation()(($) => $.helpers.presets.codexChatGPTAccount);
    },
    get description() {
        return translation()(
            ($) =>
                $.helpers.presets
                    .aChatGPTAccountOverTheCodexResponsesEndpointStreamingIsForcedUpstreamAndResponses,
        );
    },
    grant: "authorization_code",
    suggestedName: "codex",
    provenance:
        "openai/codex at rev fcf05456bb27e6c3d5677550f54011db6a2a0817 (codex-rs/login; login and inference verified against the live upstream); the compaction declarations are researched from the Codex client and backend behavior at rev 444da310 and remain unverified against a live compact run; Quota: openai/codex at 4dd51f4a5f2037f8aa322fe7807315e6530a4ec8, backend-client/src/client.rs and rate_limit_resets_tests.rs (wham usage, main/additional pools); quota remains unverified by a live query.",
    verification: "unverified",
    payload: {
        quota: {
            request: {
                ...bearer("https://chatgpt.com/backend-api/wham/usage"),
                headers: [
                    ...(bearer("").headers ?? []),
                    {
                        name: "ChatGPT-Account-Id",
                        value: [
                            {
                                type: "slot",
                                name: "account_id",
                            },
                        ],
                    },
                ],
            },
            sources: [
                {
                    type: "object",
                    key: "account",
                    at: [
                        "rate_limit",
                    ],
                    is_required: false,
                    scope: {
                        type: "literal",
                        value: "Account",
                    },
                    entries: codexWindows(),
                },
                {
                    type: "array",
                    key: "additional",
                    at: [
                        "additional_rate_limits",
                    ],
                    is_required: false,
                    identity: [
                        {
                            type: "field",
                            field: field(
                                [
                                    "metered_feature",
                                ],
                                true,
                                "item",
                            ),
                            encoding: "string",
                        },
                    ],
                    scope: {
                        type: "field",
                        field: field(
                            [
                                "limit_name",
                            ],
                            false,
                            "item",
                        ),
                        encoding: "string",
                    },
                    entries: codexWindows([
                        "rate_limit",
                    ]),
                },
            ],
        },
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
};
