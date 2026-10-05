import { translation } from "../i18n";
import { bearer, ratioEntry } from "./quota-helpers";
import type { ProviderPreset } from "./types";

/** Complete authored preset for claude-code. */
export const claudeCodePreset: ProviderPreset = {
    id: "claude-code",
    get title() {
        return translation()(($) => $.helpers.presets.claudeCodeClaudeAccount);
    },
    get description() {
        return translation()(
            ($) =>
                $.helpers.presets
                    .aClaudeSubscriptionAccountOverTheMessagesAPITheOAuthTokenRidesAuthorizationBearer,
        );
    },
    grant: "authorization_code",
    suggestedName: "claude",
    provenance:
        "anthropics/claude-code research port (hosted paste-back callback, JSON token requests); verify against the live CLI before production use; Quota: official @anthropic-ai/claude-code@2.1.0 cli.js usage reader (OAuth bearer, oauth-2025-04-20 beta, existing user:profile scope; five_hour, seven_day, seven_day_sonnet); quota remains unverified by a live query.",
    verification: "unverified",
    payload: {
        quota: {
            request: {
                ...bearer("https://api.anthropic.com/api/oauth/usage"),
                headers: [
                    ...(bearer("").headers ?? []),
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
                        name: "User-Agent",
                        value: [
                            {
                                type: "literal",
                                value: "claude-cli/2.1.0 (external, cli)",
                            },
                        ],
                    },
                ],
            },
            sources: [
                {
                    type: "object",
                    key: "account",
                    at: [],
                    is_required: true,
                    scope: {
                        type: "literal",
                        value: "Account",
                    },
                    entries: [
                        ratioEntry(
                            "five_hour",
                            "Five-hour allowance",
                            [
                                "five_hour",
                            ],
                            [
                                "utilization",
                            ],
                            {
                                type: "duration",
                                seconds: 18000,
                            },
                            [
                                "resets_at",
                            ],
                            "rfc3339",
                        ),
                        ratioEntry(
                            "seven_day",
                            "Weekly allowance",
                            [
                                "seven_day",
                            ],
                            [
                                "utilization",
                            ],
                            {
                                type: "duration",
                                seconds: 604800,
                            },
                            [
                                "resets_at",
                            ],
                            "rfc3339",
                        ),
                        ratioEntry(
                            "seven_day_sonnet",
                            "Sonnet weekly allowance",
                            [
                                "seven_day_sonnet",
                            ],
                            [
                                "utilization",
                            ],
                            {
                                type: "duration",
                                seconds: 604800,
                            },
                            [
                                "resets_at",
                            ],
                            "rfc3339",
                        ),
                    ],
                },
            ],
        },
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
};
