import { translation } from "../i18n";
import {
    authorizationCode,
    bearerAuthorization,
    clientId,
    clientSecret,
    coalesce,
    literal,
    model,
    paramRow,
    redirectUri,
    refreshToken,
    requestUuid,
    slot,
    tokenExchangeResponse,
} from "./expressions";
import { field, numberField } from "./quota-helpers";
import type { ProviderPreset } from "./types";

/** Complete authored preset for antigravity. */
export const antigravityPreset: ProviderPreset = {
    id: "antigravity",
    get title() {
        return translation()(($) => $.helpers.presets.antigravityGoogleAccount);
    },
    get description() {
        return translation()(
            ($) =>
                $.helpers.presets
                    .aGoogleAccountOverTheAntigravityGeminiEndpointWithProjectOnboardingAndAnEnveloped,
        );
    },
    grant: "authorization_code",
    suggestedName: "antigravity",
    provenance:
        "NoeFabris/opencode-antigravity-auth at rev 16e0056 (src/antigravity/oauth.ts, src/plugin); Quota: NoeFabris/opencode-antigravity-auth at 16e0056431d0a1291ee66e5938c732720b13a851, src/plugin/quota.ts and src/constants.ts (production fetchAvailableModels, stored project, quotaInfo per map key); quota remains unverified by a live query.",
    verification: "live",
    payload: {
        quota: {
            request: {
                method: "POST",
                url: [
                    literal("https://cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels"),
                ],
                headers: [
                    bearerAuthorization(),
                    paramRow("User-Agent", [
                        literal(
                            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Antigravity/1.18.3 Chrome/138.0.7204.235 Electron/37.3.1 Safari/537.36",
                        ),
                    ]),
                ],
                body: {
                    kind: "json",
                    node: {
                        type: "object",
                        fields: {
                            project: {
                                type: "string",
                                expr: [
                                    slot("project"),
                                ],
                            },
                        },
                    },
                },
            },
            sources: [
                {
                    type: "map",
                    key: "models",
                    at: [
                        "models",
                    ],
                    is_required: true,
                    scope: {
                        type: "map_key",
                    },
                    entries: [
                        {
                            key: "usage",
                            routing: {
                                type: "monitoring_only",
                            },
                            label: "Model allowance",
                            at: [
                                "quotaInfo",
                            ],
                            is_required: false,
                            window: {
                                type: "unknown",
                            },
                            measurement: {
                                type: "ratio",
                                value: numberField([
                                    "remainingFraction",
                                ]),
                                scale: "fraction",
                                direction: "remaining",
                            },
                            reset: {
                                type: "field",
                                field: field([
                                    "resetTime",
                                ]),
                                encoding: "rfc3339",
                            },
                        },
                    ],
                },
            ],
        },
        format: "gemini_generate_content",
        inference: {
            url: [
                literal("https://daily-cloudcode-pa.googleapis.com/v1internal:generateContent"),
            ],
            stream_url: [
                literal(
                    "https://daily-cloudcode-pa.googleapis.com/v1internal:streamGenerateContent?alt=sse",
                ),
            ],
            headers: [
                bearerAuthorization(),
                paramRow("User-Agent", [
                    literal("antigravity"),
                ]),
            ],
            upstream_streaming: "match_client",
            envelope: {
                type: "object",
                fields: {
                    project: {
                        type: "string",
                        expr: [
                            slot("project"),
                        ],
                    },
                    model: {
                        type: "string",
                        expr: [
                            model(),
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
                            literal("agent-"),
                            requestUuid(),
                        ],
                    },
                },
            },
            response_unwrap: "response",
        },
        credential: {
            type: "oauth",
            client_id: "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com",
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
                    literal("https://accounts.google.com/o/oauth2/v2/auth"),
                ],
                redirect_uri: "http://localhost:51121/oauth-callback",
                pkce: "none",
                extra_params: [
                    paramRow("access_type", [
                        literal("offline"),
                    ]),
                    paramRow("prompt", [
                        literal("consent"),
                    ]),
                    paramRow("scope", [
                        literal(
                            "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/userinfo.profile https://www.googleapis.com/auth/cclog https://www.googleapis.com/auth/experimentsandconfigs",
                        ),
                    ]),
                ],
            },
            exchange: {
                request: {
                    method: "POST",
                    url: [
                        literal("https://oauth2.googleapis.com/token"),
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
                            paramRow("client_id", [
                                clientId(),
                            ]),
                            paramRow("client_secret", [
                                clientSecret(),
                            ]),
                            paramRow("redirect_uri", [
                                redirectUri(),
                            ]),
                        ],
                    },
                },
                response: tokenExchangeResponse(),
                terminal_codes: [
                    "invalid_grant",
                ],
            },
            refresh: {
                type: "grant",
                request: {
                    method: "POST",
                    url: [
                        literal("https://oauth2.googleapis.com/token"),
                    ],
                    body: {
                        kind: "form",
                        fields: [
                            paramRow("grant_type", [
                                literal("refresh_token"),
                            ]),
                            paramRow("refresh_token", [
                                refreshToken(),
                            ]),
                            paramRow("client_id", [
                                clientId(),
                            ]),
                            paramRow("client_secret", [
                                clientSecret(),
                            ]),
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
                        literal(
                            "https://daily-cloudcode-pa.googleapis.com/v1internal:loadCodeAssist",
                        ),
                    ],
                    headers: [
                        bearerAuthorization(),
                        paramRow("User-Agent", [
                            literal("antigravity"),
                        ]),
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
                            literal(
                                "https://daily-cloudcode-pa.googleapis.com/v1internal:onboardUser",
                            ),
                        ],
                        headers: [
                            bearerAuthorization(),
                            paramRow("User-Agent", [
                                literal("antigravity"),
                            ]),
                        ],
                        body: {
                            kind: "json",
                            node: {
                                type: "object",
                                fields: {
                                    tierId: {
                                        type: "string",
                                        expr: [
                                            coalesce(
                                                slot("paid_tier"),
                                                slot("current_tier"),
                                                literal("free-tier"),
                                            ),
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
                    literal(
                        "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels",
                    ),
                ],
                headers: [
                    bearerAuthorization(),
                    paramRow("User-Agent", [
                        literal("antigravity"),
                    ]),
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
};
