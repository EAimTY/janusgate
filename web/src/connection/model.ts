import { appConfig } from "/src/config";

/**
 * The pair a user enters on the connection screen: the gateway's origin and
 * its management token. The token is always sent verbatim (README §3.3) and is
 * never rendered back in full.
 */
export type Connection = {
    readonly baseUrl: string;
    readonly token: string;
};

export type BaseUrlNormalization =
    | {
          readonly ok: true;
          readonly baseUrl: string;
      }
    | {
          readonly ok: false;
          readonly reason: string;
      };

/**
 * The management API's version segment. It is fixed by the generated bindings
 * (`src/management-api/generated/v0.ts`), not by the user: the route table's
 * keys are paths within this version.
 */
export const MANAGEMENT_API_PATH = "/management-api/v0";

/**
 * Normalizes the entered base URL into the origin the management API paths are
 * composed with. The input must be an `http`/`https` origin: surrounding
 * whitespace is trimmed and trailing slashes disappear so that
 * `{baseUrl}{MANAGEMENT_API_PATH}{path}` carries exactly one `/` per boundary.
 */
export function normalizeBaseUrl(rawInput: string): BaseUrlNormalization {
    const trimmedInput = rawInput.trim().replace(/\/+$/, "");
    if (trimmedInput === "") {
        return {
            ok: false,
            reason: "Enter the JanusGate base URL.",
        };
    }

    let url: URL;
    try {
        url = new URL(trimmedInput);
    } catch {
        return {
            ok: false,
            reason: "Enter an absolute URL, for example https://gateway.example.com.",
        };
    }

    if (url.protocol !== "http:" && url.protocol !== "https:") {
        return {
            ok: false,
            reason: "Use an http:// or https:// URL.",
        };
    }
    if (url.username !== "" || url.password !== "") {
        return {
            ok: false,
            reason: "Remove the credentials from the URL.",
        };
    }
    if (url.pathname !== "/" || url.search !== "" || url.hash !== "") {
        return {
            ok: false,
            reason: "Enter only the gateway origin, without a path, query, or fragment.",
        };
    }

    return {
        ok: true,
        baseUrl: url.origin,
    };
}

/**
 * Reads the stored connection pair. Anything that is not the exact stored
 * shape — malformed JSON, a foreign value, an empty field — reads as absent,
 * which sends the visitor to the connection screen.
 */
export function readStoredConnection(storage: Storage | null): Connection | null {
    if (storage === null) {
        return null;
    }

    let serialized: string | null;
    try {
        serialized = storage.getItem(appConfig.connectionStorageKey);
    } catch {
        return null;
    }
    if (serialized === null) {
        return null;
    }

    let parsed: unknown;
    try {
        parsed = JSON.parse(serialized);
    } catch {
        return null;
    }
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
        return null;
    }

    const baseUrl: unknown = Reflect.get(parsed, "baseUrl");
    const token: unknown = Reflect.get(parsed, "token");
    if (
        typeof baseUrl !== "string" ||
        baseUrl === "" ||
        typeof token !== "string" ||
        token === ""
    ) {
        return null;
    }

    return {
        baseUrl,
        token,
    };
}

/**
 * Persists the accepted pair. A storage failure (for example an exhausted
 * quota) leaves the connection usable for this session only.
 */
export function writeStoredConnection(storage: Storage | null, connection: Connection): void {
    if (storage === null) {
        return;
    }
    try {
        storage.setItem(appConfig.connectionStorageKey, JSON.stringify(connection));
    } catch {
        // The in-memory connection remains authoritative for this session.
    }
}

export function clearStoredConnection(storage: Storage | null): void {
    if (storage === null) {
        return;
    }
    try {
        storage.removeItem(appConfig.connectionStorageKey);
    } catch {
        // Nothing to recover from: the in-memory state is cleared regardless.
    }
}
