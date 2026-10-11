import { describe, expect, test } from "vitest";
import { hasBaseReference } from "./base-reference";
import type { DefinitionPayload } from "./types";

/**
 * The base-reference walk over an authored definition payload: every
 * expression surface whose evaluation environment carries the instance base
 * URL counts — directly or through a `coalesce` — across the inference
 * descriptor, the OAuth control requests, the provisioning steps and their
 * polls, the discovery request, the quota request, and the compaction
 * operations; the authorize URL and its extra parameters are excluded, the
 * write-time validation barring the base reference at that stage.
 */

/** The minimal static-key payload the mutations below start from. */
const STATIC_PAYLOAD: DefinitionPayload = {
    format: "openai_chat_completion",
    inference: {
        url: [
            {
                type: "literal",
                value: "https://api.example.test/v1/chat/completions",
            },
        ],
        upstream_streaming: "match_client",
    },
    credential: {
        type: "api_key",
    },
};

describe("hasBaseReference", () => {
    test("answers false for a payload carrying no base part", () => {
        expect(hasBaseReference(STATIC_PAYLOAD)).toBe(false);
    });

    test("reads the base part in the inference URL", () => {
        expect(
            hasBaseReference({
                ...STATIC_PAYLOAD,
                inference: {
                    ...STATIC_PAYLOAD.inference,
                    url: [
                        {
                            type: "base",
                        },
                        {
                            type: "literal",
                            value: "/chat/completions",
                        },
                    ],
                },
            }),
        ).toBe(true);
    });

    test("reads the base part through a coalesce, in headers and in the envelope", () => {
        expect(
            hasBaseReference({
                ...STATIC_PAYLOAD,
                inference: {
                    ...STATIC_PAYLOAD.inference,
                    headers: [
                        {
                            name: "X-Relay",
                            value: [
                                {
                                    type: "coalesce",
                                    parts: [
                                        {
                                            type: "base",
                                        },
                                    ],
                                },
                            ],
                        },
                    ],
                },
            }),
        ).toBe(true);

        expect(
            hasBaseReference({
                ...STATIC_PAYLOAD,
                inference: {
                    ...STATIC_PAYLOAD.inference,
                    envelope: {
                        type: "object",
                        fields: {
                            relay: {
                                type: "string",
                                expr: [
                                    {
                                        type: "coalesce",
                                        parts: [
                                            {
                                                type: "base",
                                            },
                                        ],
                                    },
                                ],
                            },
                        },
                    },
                },
            }),
        ).toBe(true);
    });

    test("reads the base part in the stream URL but not in a plain literal header", () => {
        expect(
            hasBaseReference({
                ...STATIC_PAYLOAD,
                inference: {
                    ...STATIC_PAYLOAD.inference,
                    stream_url: [
                        {
                            type: "base",
                        },
                    ],
                },
            }),
        ).toBe(true);

        expect(
            hasBaseReference({
                ...STATIC_PAYLOAD,
                inference: {
                    ...STATIC_PAYLOAD.inference,
                    headers: [
                        {
                            name: "Accept",
                            value: [
                                {
                                    type: "literal",
                                    value: "base",
                                },
                            ],
                        },
                    ],
                },
            }),
        ).toBe(false);
    });

    test("reads the base part in a discovery request", () => {
        expect(
            hasBaseReference({
                ...STATIC_PAYLOAD,
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
            }),
        ).toBe(true);
    });

    test("excludes the authorize URL and its extra parameters from the walk", () => {
        const authorizeOnlyBase: DefinitionPayload = {
            ...STATIC_PAYLOAD,
            credential: {
                type: "oauth",
                client_id: "abc",
                grant: {
                    type: "authorization_code",
                    authorize_url: [
                        {
                            type: "base",
                        },
                    ],
                    redirect_uri: "http://localhost:0/callback",
                    pkce: "none",
                },
                exchange: {
                    request: {
                        method: "POST",
                        url: [
                            {
                                type: "literal",
                                value: "https://auth.example.test/token",
                            },
                        ],
                        body: {
                            kind: "form",
                            fields: [
                                {
                                    name: "code",
                                    value: [
                                        {
                                            type: "authorization_code",
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
                        ],
                    },
                    terminal_codes: [
                        "400",
                    ],
                },
                refresh: {
                    type: "none",
                },
            },
        };

        expect(hasBaseReference(authorizeOnlyBase)).toBe(false);
    });

    test("reads the base part in an exchange request body", () => {
        const withExchangeBase: DefinitionPayload = {
            ...STATIC_PAYLOAD,
            credential: {
                type: "oauth",
                client_id: "abc",
                grant: {
                    type: "authorization_code",
                    authorize_url: [
                        {
                            type: "literal",
                            value: "https://auth.example.test",
                        },
                    ],
                    redirect_uri: "http://localhost:0/callback",
                    pkce: "none",
                },
                exchange: {
                    request: {
                        method: "POST",
                        url: [
                            {
                                type: "base",
                            },
                            {
                                type: "literal",
                                value: "/token",
                            },
                        ],
                        body: {
                            kind: "json",
                            node: {
                                type: "object",
                                fields: {
                                    code: {
                                        type: "string",
                                        expr: [
                                            {
                                                type: "authorization_code",
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
                        ],
                    },
                    terminal_codes: [
                        "400",
                    ],
                },
                refresh: {
                    type: "none",
                },
            },
        };

        expect(hasBaseReference(withExchangeBase)).toBe(true);
    });
});
