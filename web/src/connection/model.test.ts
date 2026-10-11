import { afterEach, describe, expect, test } from "vitest";
import {
    clearStoredConnection,
    normalizeBaseUrl,
    readStoredConnection,
    writeStoredConnection,
} from "./model";

/**
 * The connection pair's input and persistence: the base URL normalized to
 * the origin the management paths compose onto (trim, trailing slashes, and
 * every refusal the form names), and the stored pair read strictly —
 * anything that is not the exact stored shape reading as absent — with a
 * storage failure leaving the in-memory state usable.
 */

/** A Map-backed Storage standing in for the browser store this environment does not provide. */
function memoryStorage(): Storage {
    const entries = new Map<string, string>();

    return {
        get length(): number {
            return entries.size;
        },
        clear: (): void => {
            entries.clear();
        },
        getItem: (key: string): string | null => entries.get(key) ?? null,
        key: (index: number): string | null => Array.from(entries.keys())[index] ?? null,
        removeItem: (key: string): void => {
            entries.delete(key);
        },
        setItem: (key: string, value: string): void => {
            entries.set(key, value);
        },
    };
}

describe("normalizeBaseUrl", () => {
    test("normalizes a bare origin, trimming whitespace and trailing slashes", () => {
        expect(normalizeBaseUrl("  http://gw.test:8787///  ")).toEqual({
            ok: true,
            baseUrl: "http://gw.test:8787",
        });
    });

    test("normalizes default ports away through the URL origin", () => {
        expect(normalizeBaseUrl("https://gw.test/")).toEqual({
            ok: true,
            baseUrl: "https://gw.test",
        });
    });

    test("answers empty for blank input", () => {
        expect(normalizeBaseUrl("   ")).toEqual({
            ok: false,
            reason: "empty",
        });
    });

    test("answers not-absolute for a path-shaped address", () => {
        expect(normalizeBaseUrl("/management-api")).toEqual({
            ok: false,
            reason: "not-absolute",
        });
    });

    test("answers scheme for a non-http address or a scheme-shaped typo", () => {
        expect(normalizeBaseUrl("ftp://gw.test")).toEqual({
            ok: false,
            reason: "scheme",
        });
        // The URL parser reads `gw.test:` as a scheme, so the form refuses it there.
        expect(normalizeBaseUrl("gw.test:8787")).toEqual({
            ok: false,
            reason: "scheme",
        });
    });

    test("answers credentials for an address carrying user info", () => {
        expect(normalizeBaseUrl("http://user:pass@gw.test")).toEqual({
            ok: false,
            reason: "credentials",
        });
    });

    test("answers not-origin for a path, query, or hash", () => {
        expect(normalizeBaseUrl("http://gw.test/management-api")).toEqual({
            ok: false,
            reason: "not-origin",
        });
        expect(normalizeBaseUrl("http://gw.test/?x=1")).toEqual({
            ok: false,
            reason: "not-origin",
        });
        expect(normalizeBaseUrl("http://gw.test/#setup")).toEqual({
            ok: false,
            reason: "not-origin",
        });
    });
});

describe("stored connection persistence", () => {
    afterEach(() => {
        window.localStorage?.clear();
    });

    test("reads back the pair writeStoredConnection persisted", () => {
        const storage = memoryStorage();

        writeStoredConnection(storage, {
            baseUrl: "http://gw.test:8787",
            token: "t-1",
        });

        expect(readStoredConnection(storage)).toEqual({
            baseUrl: "http://gw.test:8787",
            token: "t-1",
        });
        expect(storage.getItem("janusgate-web:connection")).toBe(
            JSON.stringify({
                baseUrl: "http://gw.test:8787",
                token: "t-1",
            }),
        );
    });

    test("reads absence, a foreign value, a broken shape, and blank fields as null", () => {
        const storage = memoryStorage();

        expect(readStoredConnection(storage)).toBeNull();

        storage.setItem("janusgate-web:connection", "not json");
        expect(readStoredConnection(storage)).toBeNull();

        storage.setItem(
            "janusgate-web:connection",
            JSON.stringify([
                "a",
                "b",
            ]),
        );
        expect(readStoredConnection(storage)).toBeNull();

        storage.setItem(
            "janusgate-web:connection",
            JSON.stringify({
                baseUrl: "http://gw.test",
                token: "",
            }),
        );
        expect(readStoredConnection(storage)).toBeNull();

        storage.setItem(
            "janusgate-web:connection",
            JSON.stringify({
                baseUrl: "",
                token: "t",
            }),
        );
        expect(readStoredConnection(storage)).toBeNull();

        storage.setItem(
            "janusgate-web:connection",
            JSON.stringify({
                token: "t",
            }),
        );
        expect(readStoredConnection(storage)).toBeNull();
    });

    test("answers null for a null storage without touching anything", () => {
        expect(readStoredConnection(null)).toBeNull();
        expect(() =>
            writeStoredConnection(null, {
                baseUrl: "http://gw.test",
                token: "t",
            }),
        ).not.toThrow();
        expect(() => clearStoredConnection(null)).not.toThrow();
    });

    test("clears the stored pair", () => {
        const storage = memoryStorage();
        writeStoredConnection(storage, {
            baseUrl: "http://gw.test",
            token: "t",
        });

        clearStoredConnection(storage);

        expect(readStoredConnection(storage)).toBeNull();
    });

    test("keeps the in-memory state usable when storage refuses writes", () => {
        const refusing: Storage = {
            ...memoryStorage(),
            setItem: () => {
                throw new Error("quota exceeded");
            },
        };

        expect(() =>
            writeStoredConnection(refusing, {
                baseUrl: "http://gw.test",
                token: "t",
            }),
        ).not.toThrow();
    });

    test("reads absent when storage refuses reads", () => {
        const refusing: Storage = {
            ...memoryStorage(),
            getItem: () => {
                throw new Error("denied");
            },
        };

        expect(readStoredConnection(refusing)).toBeNull();
    });

    test("treats a clear refused by storage as done", () => {
        const refusing: Storage = {
            ...memoryStorage(),
            removeItem: () => {
                throw new Error("denied");
            },
        };

        expect(() => clearStoredConnection(refusing)).not.toThrow();
    });
});
