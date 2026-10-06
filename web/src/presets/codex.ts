import { translation } from "../i18n";
import type { QuotaEntryExtraction } from "../management-api/generated/v0";
import {
    authorizationCode,
    bearerAuthorization,
    clientId,
    codeVerifier,
    jsonRefreshGrantBody,
    literal,
    paramRow,
    redirectUri,
    requestUuid,
    responseField,
    slot,
    tokenExchangeResponse,
} from "./expressions";
import { field, numberField, ratioEntry } from "./quota-helpers";
import type { ProviderPreset } from "./types";

/** Codex uses upstream durations; window names do not imply five hours or seven days. */
function codexWindows(at: string[] = []): QuotaEntryExtraction[] {
    return [
        "primary",
        "secondary",
    ].map((key) =>
        ratioEntry(
            key,
            key === "primary" ? "Primary window" : "Secondary window",
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
                method: "GET",
                url: [
                    literal("https://chatgpt.com/backend-api/wham/usage"),
                ],
                headers: [
                    bearerAuthorization(),
                    paramRow("ChatGPT-Account-Id", [
                        slot("account_id"),
                    ]),
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
                literal("https://chatgpt.com/backend-api/codex/responses"),
            ],
            headers: [
                bearerAuthorization(),
                paramRow("chatgpt-account-id", [
                    slot("account_id"),
                ]),
                paramRow("Originator", [
                    literal("codex_cli_rs"),
                ]),
                paramRow("User-Agent", [
                    literal("codex_cli_rs/0.147.0"),
                ]),
                paramRow("Session-Id", [
                    requestUuid(),
                ]),
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
                    literal("https://auth.openai.com/oauth/authorize"),
                ],
                redirect_uri: "http://localhost:1455/auth/callback",
                pkce: "s256",
                extra_params: [
                    paramRow("scope", [
                        literal(
                            "openid profile email offline_access api.connectors.read api.connectors.invoke",
                        ),
                    ]),
                    paramRow("id_token_add_organizations", [
                        literal("true"),
                    ]),
                    paramRow("codex_cli_simplified_flow", [
                        literal("true"),
                    ]),
                    paramRow("originator", [
                        literal("codex_cli_rs"),
                    ]),
                ],
            },
            exchange: {
                request: {
                    method: "POST",
                    url: [
                        literal("https://auth.openai.com/oauth/token"),
                    ],
                    body: {
                        kind: "form",
                        fields: [
                            paramRow("grant_type", [
                                literal("authorization_code"),
                            ]),
                            paramRow("code", [
                                authorizationCode(),
                            ]),
                            paramRow("redirect_uri", [
                                redirectUri(),
                            ]),
                            paramRow("client_id", [
                                clientId(),
                            ]),
                            paramRow("code_verifier", [
                                codeVerifier(),
                            ]),
                        ],
                    },
                },
                response: tokenExchangeResponse(
                    responseField(
                        "id_token",
                        {
                            type: "id_token",
                        },
                        true,
                    ),
                ),
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
                        literal("https://auth.openai.com/oauth/token"),
                    ],
                    headers: [
                        paramRow("User-Agent", [
                            literal("codex_cli_rs/0.147.0"),
                        ]),
                    ],
                    body: jsonRefreshGrantBody(),
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
                    literal("https://chatgpt.com/backend-api/codex/models"),
                ],
                query: [
                    paramRow("client_version", [
                        literal("0.147.0"),
                    ]),
                ],
                headers: [
                    bearerAuthorization(),
                    paramRow("chatgpt-account-id", [
                        slot("account_id"),
                    ]),
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
