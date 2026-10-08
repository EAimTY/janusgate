import type {
    ControlBody,
    Discovery,
    ParamRow,
    ResponseDescriptor,
    ResponseField,
    ResponseTarget,
    StringExpr,
    ValueExpr,
} from "../management-api/generated/v0";

/**
 * Constructors for the preset catalog's definition payloads: one function per
 * expression-node kind the presets use (README §5.1), the param row every
 * header, query, and form surface spells, the shared request and OAuth-stage
 * shapes, and the per-family discovery descriptors. Product modules compose
 * payloads through these and keep only their pinned facts — URLs, betas,
 * terminal codes, extract paths; the constructors return the generated wire
 * types, so the bindings type-check every assembled node.
 */

/** A `literal` expression node: the constant text. */
export function literal(value: string): ValueExpr {
    return {
        type: "literal",
        value,
    };
}

/** A `secret` expression node: the definition's active credential secret. */
export function secret(): ValueExpr {
    return {
        type: "secret",
    };
}

/** A `slot` expression node: the named flow slot's stored value. */
export function slot(name: string): ValueExpr {
    return {
        type: "slot",
        name,
    };
}

/** A `base` expression node: the instance's configured base URL. */
export function base(): ValueExpr {
    return {
        type: "base",
    };
}

/** A `model` expression node: the requested model identifier. */
export function model(): ValueExpr {
    return {
        type: "model",
    };
}

/** A `request_uuid` expression node: a fresh UUID minted per request. */
export function requestUuid(): ValueExpr {
    return {
        type: "request_uuid",
    };
}

/** A `client_id` expression node: the OAuth credential's client id. */
export function clientId(): ValueExpr {
    return {
        type: "client_id",
    };
}

/** A `client_secret` expression node: the OAuth credential's client secret. */
export function clientSecret(): ValueExpr {
    return {
        type: "client_secret",
    };
}

/** An `authorization_code` expression node: the grant's received code. */
export function authorizationCode(): ValueExpr {
    return {
        type: "authorization_code",
    };
}

/** A `code_verifier` expression node: the PKCE verifier of the flow. */
export function codeVerifier(): ValueExpr {
    return {
        type: "code_verifier",
    };
}

/** A `redirect_uri` expression node: the flow's registered redirect URI. */
export function redirectUri(): ValueExpr {
    return {
        type: "redirect_uri",
    };
}

/** A `device_code` expression node: the device grant's polling code. */
function deviceCode(): ValueExpr {
    return {
        type: "device_code",
    };
}

/** A `refresh_token` expression node: the stored refresh token. */
function refreshToken(): ValueExpr {
    return {
        type: "refresh_token",
    };
}

/** A `coalesce` expression node: the parts tried in order until one answers. */
export function coalesce(...parts: ValueExpr[]): ValueExpr {
    return {
        type: "coalesce",
        parts,
    };
}

/** One header, query, or form-field param row carrying its evaluated value. */
export function paramRow(name: string, value: StringExpr): ParamRow {
    return {
        name,
        value,
    };
}

/**
 * The bearer `Authorization` row: the fixed scheme before the definition's
 * active secret. Requests reusing it beside additional rows list it first.
 */
export function bearerAuthorization(): ParamRow {
    return paramRow("Authorization", [
        literal("Bearer "),
        secret(),
    ]);
}

/**
 * The OpenAI-style model list: a bearer GET whose response carries `data[].id`
 * and fits one response. `headers` replace the default lone Authorization row —
 * the researched per-product deltas (an extra integration header, or a
 * static-key header row where the endpoint does not take the secret bearer).
 */
export function openAiModelsDiscovery(
    url: StringExpr,
    headers: readonly ParamRow[] = [
        bearerAuthorization(),
    ],
): Discovery {
    return {
        request: {
            method: "GET",
            url,
            headers: [
                ...headers,
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
    };
}

/**
 * The Anthropic model list: `data[].id` walked by the `has_more`/`last_id`
 * cursor. The `anthropic-version` row is pinned; `authHeaders` carry the
 * family's credential style — the static-key `x-api-key` row, or the OAuth
 * bearer row beside its beta.
 */
export function anthropicModelsDiscovery(
    url: StringExpr,
    authHeaders: readonly ParamRow[],
): Discovery {
    return {
        request: {
            method: "GET",
            url,
            headers: [
                ...authHeaders,
                paramRow("anthropic-version", [
                    literal("2023-06-01"),
                ]),
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
    };
}

/**
 * The Gemini model list: the `models/<id>` resource names walked by the
 * `pageToken`/`nextPageToken` loop at the API's 1000-model page size. The
 * static-key `x-goog-api-key` row is pinned; `extraHeaders` carry the
 * per-endpoint deltas.
 */
export function geminiModelsDiscovery(
    url: StringExpr,
    extraHeaders: readonly ParamRow[] = [],
): Discovery {
    return {
        request: {
            method: "GET",
            url,
            query: [
                paramRow("pageSize", [
                    literal("1000"),
                ]),
            ],
            headers: [
                paramRow("x-goog-api-key", [
                    secret(),
                ]),
                ...extraHeaders,
            ],
        },
        extract: {
            mode: "array",
            array_at: [
                "models",
            ],
            id_field: [
                "name",
            ],
            strip_prefix: "models/",
        },
        pagination: {
            type: "token",
            request_param: "pageToken",
            response_path: [
                "nextPageToken",
            ],
            max_pages: 10,
        },
    };
}

/** One decoded control-response mapping: the top-level member into its target. */
export function responseField(
    path: string,
    target: ResponseTarget,
    isRequired: boolean,
): ResponseField {
    return {
        path: [
            path,
        ],
        target,
        required: isRequired,
    };
}

/**
 * The JSON token-exchange response the authorization-code presets decode: both
 * tokens required and the optional expiry last. `extra` carries the researched
 * delta — Codex's required `id_token` rides before the expiry.
 */
export function tokenExchangeResponse(...extra: readonly ResponseField[]): ResponseDescriptor {
    return {
        encoding: "json",
        fields: [
            responseField(
                "access_token",
                {
                    type: "access_token",
                },
                true,
            ),
            responseField(
                "refresh_token",
                {
                    type: "refresh_token",
                },
                true,
            ),
            ...extra,
            responseField(
                "expires_in",
                {
                    type: "expires_in_seconds",
                },
                false,
            ),
        ],
    };
}

/**
 * The JSON device-authorization response: both codes required, and the
 * verification URIs plus the polling facts optional. `withCompleteUri` adds the
 * pre-built `verification_uri_complete` member the endpoints that carry it
 * report ahead of the plain URI.
 */
export function deviceAuthorizationResponse(withCompleteUri: boolean): ResponseDescriptor {
    return {
        encoding: "json",
        fields: [
            responseField(
                "device_code",
                {
                    type: "device_code",
                },
                true,
            ),
            responseField(
                "user_code",
                {
                    type: "user_code",
                },
                true,
            ),
            ...(withCompleteUri
                ? [
                      responseField(
                          "verification_uri_complete",
                          {
                              type: "verification_uri_complete",
                          },
                          false,
                      ),
                  ]
                : []),
            responseField(
                "verification_uri",
                {
                    type: "verification_uri",
                },
                false,
            ),
            responseField(
                "expires_in",
                {
                    type: "expires_in_seconds",
                },
                false,
            ),
            responseField(
                "interval",
                {
                    type: "interval_seconds",
                },
                false,
            ),
        ],
    };
}

/** The device-authorization request body: the client identity as the lone form field. */
export function deviceAuthorizationBody(): ControlBody {
    return {
        kind: "form",
        fields: [
            paramRow("client_id", [
                clientId(),
            ]),
        ],
    };
}

/**
 * The device-grant exchange body: the RFC 8628 grant-type URN, the client
 * identity, and the polling code, form-encoded.
 */
export function deviceExchangeBody(): ControlBody {
    return {
        kind: "form",
        fields: [
            paramRow("grant_type", [
                literal("urn:ietf:params:oauth:grant-type:device_code"),
            ]),
            paramRow("client_id", [
                clientId(),
            ]),
            paramRow("device_code", [
                deviceCode(),
            ]),
        ],
    };
}

/**
 * The JSON refresh-grant body: the standard grant type, the client identity,
 * and the stored refresh token.
 */
export function jsonRefreshGrantBody(): ControlBody {
    return {
        kind: "json",
        node: {
            type: "object",
            fields: {
                grant_type: {
                    type: "string",
                    expr: [
                        literal("refresh_token"),
                    ],
                },
                client_id: {
                    type: "string",
                    expr: [
                        clientId(),
                    ],
                },
                refresh_token: {
                    type: "string",
                    expr: [
                        refreshToken(),
                    ],
                },
            },
        },
    };
}

/**
 * The form-encoded refresh-grant body: the standard grant type, the stored
 * refresh token, and the client identity. `hasClientSecret` adds the
 * confidential-client `client_secret` row after the identity — the delta of
 * the endpoints that authenticate the client on the refresh too.
 */
export function formRefreshGrantBody(hasClientSecret: boolean): ControlBody {
    return {
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
            ...(hasClientSecret
                ? [
                      paramRow("client_secret", [
                          clientSecret(),
                      ]),
                  ]
                : []),
        ],
    };
}
