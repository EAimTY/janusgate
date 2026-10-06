import { translation } from "../i18n";
import {
    base,
    bearerAuthorization,
    deviceAuthorizationBody,
    deviceAuthorizationResponse,
    deviceExchangeBody,
    literal,
    openAiModelsDiscovery,
    paramRow,
    slot,
} from "./expressions";
import { ratioEntry } from "./quota-helpers";
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
                    literal("https://api.github.com/copilot_internal/user"),
                ],
                headers: [
                    paramRow("Authorization", [
                        literal("token "),
                        slot("github_token"),
                    ]),
                    paramRow("X-GitHub-Api-Version", [
                        literal("2025-04-01"),
                    ]),
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
                        ratioEntry({
                            key: "allowance",
                            label: "Allowance",
                            at: [],
                            isRequired: true,
                            window: {
                                type: "unknown",
                            },
                            valuePath: [
                                "percent_remaining",
                            ],
                            isValueRequired: true,
                            direction: "remaining",
                            unlimitedPath: [
                                "unlimited",
                            ],
                            resetPath: [
                                "quota_reset_date",
                            ],
                            resetEncoding: "date",
                            resetBase: "root",
                        }),
                    ],
                },
            ],
        },
        format: "openai_chat_completion",
        inference: {
            url: [
                base(),
                literal("/chat/completions"),
            ],
            headers: [
                bearerAuthorization(),
                paramRow("Copilot-Integration-Id", [
                    literal("vscode-chat"),
                ]),
                paramRow("Editor-Version", [
                    literal("vscode/1.99.0"),
                ]),
                paramRow("User-Agent", [
                    literal("GitHubCopilotChat/1.0.0"),
                ]),
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
                            literal("https://github.com/login/device/code"),
                        ],
                        headers: [
                            paramRow("Accept", [
                                literal("application/json"),
                            ]),
                        ],
                        body: deviceAuthorizationBody(),
                    },
                    response: deviceAuthorizationResponse(false),
                },
                expires_in_fallback_seconds: 900,
            },
            exchange: {
                request: {
                    method: "POST",
                    url: [
                        literal("https://github.com/login/oauth/access_token"),
                    ],
                    headers: [
                        paramRow("Accept", [
                            literal("application/json"),
                        ]),
                    ],
                    body: deviceExchangeBody(),
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
                        literal("https://api.github.com/copilot_internal/v2/token"),
                    ],
                    headers: [
                        paramRow("Authorization", [
                            literal("Bearer "),
                            slot("github_token"),
                        ]),
                        paramRow("Editor-Version", [
                            literal("vscode/1.99.0"),
                        ]),
                        paramRow("Copilot-Integration-Id", [
                            literal("vscode-chat"),
                        ]),
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
        discovery: openAiModelsDiscovery(
            [
                base(),
                literal("/models"),
            ],
            [
                bearerAuthorization(),
                paramRow("Copilot-Integration-Id", [
                    literal("vscode-chat"),
                ]),
            ],
        ),
    },
};
