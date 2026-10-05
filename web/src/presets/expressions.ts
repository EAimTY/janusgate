import type {
    ControlRequest,
    ParamRow,
    StringExpr,
    ValueExpr,
} from "../management-api/generated/v0";

/**
 * Constructors for the preset catalog's definition payloads: one function per
 * expression-node kind the presets use (README §5.1), the param row every
 * header, query, and form surface spells, and the one shared bearer request.
 * Product modules compose payloads through these; the constructors return the
 * generated wire types, so the bindings type-check every assembled node.
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
export function deviceCode(): ValueExpr {
    return {
        type: "device_code",
    };
}

/** A `refresh_token` expression node: the stored refresh token. */
export function refreshToken(): ValueExpr {
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

/** Standard current-token bearer request, with headers entirely in preset data. */
export function bearer(url: string): ControlRequest {
    return {
        method: "GET",
        url: [
            literal(url),
        ],
        headers: [
            paramRow("Authorization", [
                literal("Bearer "),
                secret(),
            ]),
        ],
    };
}
