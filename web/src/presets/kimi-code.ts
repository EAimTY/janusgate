import { translation } from "../i18n";
import {
    bearerAuthorization,
    deviceAuthorizationBody,
    deviceAuthorizationResponse,
    deviceExchangeBody,
    formRefreshGrantBody,
    literal,
    openAiModelsDiscovery,
    paramRow,
    slot,
    tokenExchangeResponse,
} from "./expressions";
import { ratioEntry } from "./quota-helpers";
import type { ProviderPreset } from "./types";

/**
 * The pinned X-Msh client header set every Kimi Code request carries: the
 * stable device slot, the platform, and the CLI user agent.
 */
const mshHeaders = [
    paramRow("X-Msh-Device-Id", [
        slot("device_id"),
    ]),
    paramRow("X-Msh-Platform", [
        literal("macos"),
    ]),
    paramRow("User-Agent", [
        literal("KimiCLI/1.0.0"),
    ]),
];

/** Complete authored preset for kimi-code. */
export const kimiCodePreset: ProviderPreset = {
    id: "kimi-code",
    get title() {
        return translation()(($) => $.helpers.presets.kimiCodeKimiAccount);
    },
    get description() {
        return translation()(($) => $.linkedMessages.kimiDescription);
    },
    grant: "device",
    suggestedName: "kimi",
    provenance:
        "Kimi Code CLI research port (auth.kimi.com device flow, X-Msh-* header set); verify against the live CLI before production use; Quota: MoonshotAI/kimi-code at 21406fb4c805cc8c715e6d1f16ad3fb5f25f4fe3, packages/oauth/src/managed-usage.ts (four independent usages, explicit number/string union); quota remains unverified by a live query.",
    verification: "unverified",
    payload: {
        quota: {
            request: {
                method: "GET",
                url: [
                    literal("https://api.kimi.com/coding/v1/usages"),
                ],
                headers: [
                    bearerAuthorization(),
                    paramRow("Accept", [
                        literal("application/json"),
                    ]),
                ],
            },
            sources: [
                {
                    type: "object",
                    key: "account",
                    at: [
                        "usages",
                    ],
                    is_required: true,
                    scope: {
                        type: "literal",
                        value: "Account",
                    },
                    entries: [
                        ratioEntry({
                            key: "limit_5h",
                            label: "Five-hour allowance",
                            at: [
                                "limit_5h",
                            ],
                            valuePath: [
                                "used_ratio",
                            ],
                            window: {
                                type: "duration",
                                seconds: 18000,
                            },
                            resetPath: [
                                "reset_time",
                            ],
                            resetEncoding: "rfc3339",
                            encoding: "number_or_numeric_string",
                            scale: "fraction",
                        }),
                        ratioEntry({
                            key: "limit_7d",
                            label: "Weekly allowance",
                            at: [
                                "limit_7d",
                            ],
                            valuePath: [
                                "used_ratio",
                            ],
                            window: {
                                type: "duration",
                                seconds: 604800,
                            },
                            resetPath: [
                                "reset_time",
                            ],
                            resetEncoding: "rfc3339",
                            encoding: "number_or_numeric_string",
                            scale: "fraction",
                        }),
                        ratioEntry({
                            key: "limit_month_total",
                            label: "Monthly total",
                            at: [
                                "limit_month_total",
                            ],
                            valuePath: [
                                "used_ratio",
                            ],
                            window: {
                                type: "calendar",
                                unit: "month",
                            },
                            resetPath: [
                                "reset_time",
                            ],
                            resetEncoding: "rfc3339",
                            encoding: "number_or_numeric_string",
                            scale: "fraction",
                        }),
                        ratioEntry({
                            key: "limit_month_code",
                            label: "Monthly coding",
                            at: [
                                "limit_month_code",
                            ],
                            valuePath: [
                                "used_ratio",
                            ],
                            window: {
                                type: "calendar",
                                unit: "month",
                            },
                            resetPath: [
                                "reset_time",
                            ],
                            resetEncoding: "rfc3339",
                            encoding: "number_or_numeric_string",
                            scale: "fraction",
                        }),
                    ],
                },
            ],
        },
        format: "openai_chat_completion",
        inference: {
            url: [
                literal("https://api.kimi.com/coding/v1"),
                literal("/chat/completions"),
            ],
            headers: [
                bearerAuthorization(),
                ...mshHeaders,
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
                            literal("https://auth.kimi.com/oauth/device_authorization"),
                        ],
                        headers: [
                            ...mshHeaders,
                        ],
                        body: deviceAuthorizationBody(),
                    },
                    response: deviceAuthorizationResponse(true),
                },
                expires_in_fallback_seconds: 900,
            },
            exchange: {
                request: {
                    method: "POST",
                    url: [
                        literal("https://auth.kimi.com/oauth/token"),
                    ],
                    headers: [
                        ...mshHeaders,
                    ],
                    body: deviceExchangeBody(),
                },
                response: tokenExchangeResponse(),
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
                        literal("https://auth.kimi.com/oauth/token"),
                    ],
                    headers: [
                        ...mshHeaders,
                    ],
                    body: formRefreshGrantBody(false),
                },
                terminal_codes: [
                    "invalid_grant",
                ],
            },
        },
        discovery: openAiModelsDiscovery([
            literal("https://api.kimi.com/coding/v1/models"),
        ]),
    },
};
