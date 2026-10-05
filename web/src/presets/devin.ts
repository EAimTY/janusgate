import { translation } from "../i18n";
import {
    authorizationCode,
    base,
    clientId,
    codeVerifier,
    literal,
    paramRow,
    redirectUri,
    secret,
} from "./expressions";
import type { ProviderPreset } from "./types";

/** Complete authored preset for devin. */
export const devinPreset: ProviderPreset = {
    id: "devin",
    get title() {
        return translation()(($) => $.helpers.presets.devinDevinAccount);
    },
    get description() {
        return translation()(
            ($) =>
                $.helpers.presets
                    .aDevinAccountWhoseConnectRPCTokenExchangeAnswersAStaticAPIKeyAndAPI,
        );
    },
    grant: "authorization_code",
    suggestedName: "devin",
    provenance:
        "Devin OAuth research port (ConnectRPC exchange, api_key/api_server_url fields); the exchange fields and inference protocol carry research-level uncertainty — record a live confirmation before production use; quota absent: steipete/CodexBar at 0ac27e8f78801cb893b31e220fce8589241f4c3c, docs/devin.md; the researched app.devin.ai billing quota read requires browser-session auth and an internal organization identity not provided by this preset; public API consumption is ACU accounting and does not establish coding-plan allowance access under these credentials",
    verification: "unverified",
    payload: {
        format: "anthropic_messages",
        inference: {
            url: [
                base(),
                literal("/v1/messages"),
            ],
            headers: [
                paramRow("Authorization", [
                    literal("Bearer "),
                    secret(),
                ]),
                paramRow("anthropic-version", [
                    literal("2023-06-01"),
                ]),
            ],
            upstream_streaming: "match_client",
        },
        credential: {
            type: "oauth",
            client_id: "devin-oauth-client",
            grant: {
                type: "authorization_code",
                authorize_url: [
                    literal("https://app.devin.ai/oauth/authorize"),
                ],
                redirect_uri: "http://localhost:1455/auth/callback",
                pkce: "s256",
                extra_params: [],
            },
            exchange: {
                request: {
                    method: "POST",
                    url: [
                        literal("https://api.devin.ai/v1/oauth/exchange_token"),
                    ],
                    headers: [
                        paramRow("connect-protocol-version", [
                            literal("1"),
                        ]),
                    ],
                    body: {
                        kind: "json",
                        node: {
                            type: "object",
                            fields: {
                                grantType: {
                                    type: "string",
                                    expr: [
                                        literal("authorization_code"),
                                    ],
                                },
                                clientId: {
                                    type: "string",
                                    expr: [
                                        clientId(),
                                    ],
                                },
                                code: {
                                    type: "string",
                                    expr: [
                                        authorizationCode(),
                                    ],
                                },
                                codeVerifier: {
                                    type: "string",
                                    expr: [
                                        codeVerifier(),
                                    ],
                                },
                                redirectUri: {
                                    type: "string",
                                    expr: [
                                        redirectUri(),
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
};
