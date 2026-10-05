import { translation } from "../i18n";
import { bearer, numberField, ratioEntry } from "./quota-helpers";
import type { ProviderPreset } from "./types";

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
                ...bearer("https://api.kimi.com/coding/v1/usages"),
                headers: [
                    ...(bearer("").headers ?? []),
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
                        {
                            ...ratioEntry(
                                "limit_5h",
                                "Five-hour allowance",
                                [
                                    "limit_5h",
                                ],
                                [
                                    "used_ratio",
                                ],
                                {
                                    type: "duration",
                                    seconds: 18000,
                                },
                                [
                                    "reset_time",
                                ],
                                "rfc3339",
                                "number_or_numeric_string",
                            ),
                            measurement: {
                                type: "ratio",
                                value: numberField(
                                    [
                                        "used_ratio",
                                    ],
                                    "number_or_numeric_string",
                                ),
                                scale: "fraction",
                                direction: "used",
                            },
                        },
                        {
                            ...ratioEntry(
                                "limit_7d",
                                "Weekly allowance",
                                [
                                    "limit_7d",
                                ],
                                [
                                    "used_ratio",
                                ],
                                {
                                    type: "duration",
                                    seconds: 604800,
                                },
                                [
                                    "reset_time",
                                ],
                                "rfc3339",
                                "number_or_numeric_string",
                            ),
                            measurement: {
                                type: "ratio",
                                value: numberField(
                                    [
                                        "used_ratio",
                                    ],
                                    "number_or_numeric_string",
                                ),
                                scale: "fraction",
                                direction: "used",
                            },
                        },
                        {
                            ...ratioEntry(
                                "limit_month_total",
                                "Monthly total",
                                [
                                    "limit_month_total",
                                ],
                                [
                                    "used_ratio",
                                ],
                                {
                                    type: "calendar",
                                    unit: "month",
                                },
                                [
                                    "reset_time",
                                ],
                                "rfc3339",
                                "number_or_numeric_string",
                            ),
                            measurement: {
                                type: "ratio",
                                value: numberField(
                                    [
                                        "used_ratio",
                                    ],
                                    "number_or_numeric_string",
                                ),
                                scale: "fraction",
                                direction: "used",
                            },
                        },
                        {
                            ...ratioEntry(
                                "limit_month_code",
                                "Monthly coding",
                                [
                                    "limit_month_code",
                                ],
                                [
                                    "used_ratio",
                                ],
                                {
                                    type: "calendar",
                                    unit: "month",
                                },
                                [
                                    "reset_time",
                                ],
                                "rfc3339",
                                "number_or_numeric_string",
                            ),
                            measurement: {
                                type: "ratio",
                                value: numberField(
                                    [
                                        "used_ratio",
                                    ],
                                    "number_or_numeric_string",
                                ),
                                scale: "fraction",
                                direction: "used",
                            },
                        },
                    ],
                },
            ],
        },
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
};
