import { translation } from "../i18n";
import { field, numberField } from "./quota-helpers";
import type { ProviderPreset } from "./types";

/** Complete authored preset for github-copilot. */
export const githubCopilotPreset: ProviderPreset = {
    id: "github-copilot",
    get title() {
        return translation()(($) => $.helpers.presets.githubCopilotGitHubAccount);
    },
    get description() {
        return translation()(($) => $.linkedMessages.githubDescription);
    },
    grant: "device",
    suggestedName: "copilot",
    provenance:
        "github/copilot-cli and VS Code Copilot Chat research port (device flow, copilot_internal/v2/token); verify against the live product before production use; Quota: microsoft/vscode at de892dd1f9971e3a7d8c758a8788d86854047605 product.json and microsoft/vscode-copilot-chat at 5863f5a7088958050792b5dccbe8b46c6e13eccc, copilotTokenManager.ts and chatQuotaService.ts (durable github_token, explicit unlimited, percent_remaining, date-only quota_reset_date); quota remains unverified by a live query.",
    verification: "unverified",
    payload: {
        quota: {
            request: {
                method: "GET",
                url: [
                    {
                        type: "literal",
                        value: "https://api.github.com/copilot_internal/user",
                    },
                ],
                headers: [
                    {
                        name: "Authorization",
                        value: [
                            {
                                type: "literal",
                                value: "token ",
                            },
                            {
                                type: "slot",
                                name: "github_token",
                            },
                        ],
                    },
                    {
                        name: "X-GitHub-Api-Version",
                        value: [
                            {
                                type: "literal",
                                value: "2025-04-01",
                            },
                        ],
                    },
                ],
            },
            sources: [
                {
                    type: "map",
                    key: "allowances",
                    at: [
                        "quota_snapshots",
                    ],
                    is_required: false,
                    scope: {
                        type: "map_key",
                    },
                    entries: [
                        {
                            key: "allowance",
                            routing: {
                                type: "monitoring_only",
                            },
                            get label() {
                                return translation()(($) => $.helpers.presetQuotas.allowance);
                            },
                            at: [],
                            is_required: true,
                            window: {
                                type: "unknown",
                            },
                            is_unlimited: field(
                                [
                                    "unlimited",
                                ],
                                true,
                            ),
                            measurement: {
                                type: "ratio",
                                value: numberField(
                                    [
                                        "percent_remaining",
                                    ],
                                    "number",
                                    true,
                                ),
                                scale: "percent",
                                direction: "remaining",
                            },
                            reset: {
                                type: "field",
                                field: field(
                                    [
                                        "quota_reset_date",
                                    ],
                                    false,
                                    "root",
                                ),
                                encoding: "date",
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
};
