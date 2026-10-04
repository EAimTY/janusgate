import { describe, expect, it } from "vitest";
import {
    clearStoredConnection,
    normalizeBaseUrl,
    readStoredConnection,
    writeStoredConnection,
} from "./model";

describe("normalizeBaseUrl", () => {
    it("trims surrounding whitespace and strips trailing slashes", () => {
        expect(normalizeBaseUrl("  https://gateway.example.com/  ")).toEqual({
            ok: true,
            baseUrl: "https://gateway.example.com",
        });

        expect(normalizeBaseUrl("http://localhost:47182//")).toEqual({
            ok: true,
            baseUrl: "http://localhost:47182",
        });
    });

    it("keeps an explicit non-default port and drops the default one", () => {
        expect(normalizeBaseUrl("http://gateway.example.com:8080")).toEqual({
            ok: true,
            baseUrl: "http://gateway.example.com:8080",
        });

        expect(normalizeBaseUrl("https://gateway.example.com:443")).toEqual({
            ok: true,
            baseUrl: "https://gateway.example.com",
        });
    });

    it("normalizes the scheme and host case", () => {
        expect(normalizeBaseUrl("HTTPS://Gateway.Example.COM")).toEqual({
            ok: true,
            baseUrl: "https://gateway.example.com",
        });
    });

    it("rejects an empty input", () => {
        expect(normalizeBaseUrl("   ")).toEqual({
            ok: false,
            reason: "empty",
        });
    });

    it("rejects input that is not an absolute URL", () => {
        expect(normalizeBaseUrl("gateway.example.com")).toEqual({
            ok: false,
            reason: "not-absolute",
        });
    });

    it("rejects a non-http scheme", () => {
        expect(normalizeBaseUrl("ftp://gateway.example.com")).toEqual({
            ok: false,
            reason: "scheme",
        });
    });

    it("rejects embedded credentials", () => {
        expect(normalizeBaseUrl("https://user:secret@gateway.example.com")).toEqual({
            ok: false,
            reason: "credentials",
        });
    });

    it("rejects anything but an origin", () => {
        const reason = "not-origin";

        expect(normalizeBaseUrl("https://gateway.example.com/management-api/v0")).toEqual({
            ok: false,
            reason,
        });

        expect(normalizeBaseUrl("https://gateway.example.com/?debug=1")).toEqual({
            ok: false,
            reason,
        });

        expect(normalizeBaseUrl("https://gateway.example.com/#setup")).toEqual({
            ok: false,
            reason,
        });
    });
});

describe("stored connection", () => {
    const createStorage = (): Storage => {
        const entries = new Map<string, string>();

        return {
            get length() {
                return entries.size;
            },
            clear: () => entries.clear(),
            getItem: (key: string) => entries.get(key) ?? null,
            key: (index: number) =>
                [
                    ...entries.keys(),
                ][index] ?? null,
            removeItem: (key: string) => {
                entries.delete(key);
            },
            setItem: (key: string, value: string) => {
                entries.set(key, value);
            },
        };
    };

    it("round-trips a connection and clears it", () => {
        const storage = createStorage();
        expect(readStoredConnection(storage)).toBeNull();

        writeStoredConnection(storage, {
            baseUrl: "https://gateway.example.com",
            token: "secret",
        });

        expect(readStoredConnection(storage)).toEqual({
            baseUrl: "https://gateway.example.com",
            token: "secret",
        });

        clearStoredConnection(storage);
        expect(readStoredConnection(storage)).toBeNull();
    });

    it("treats a corrupt stored value as absent", () => {
        const storage = createStorage();
        storage.setItem("janusgate-web:connection", "not json");
        expect(readStoredConnection(storage)).toBeNull();
    });

    it("treats a stored value that is not a connection as absent", () => {
        const storage = createStorage();

        storage.setItem(
            "janusgate-web:connection",
            JSON.stringify({
                baseUrl: 42,
            }),
        );

        expect(readStoredConnection(storage)).toBeNull();

        storage.setItem(
            "janusgate-web:connection",
            JSON.stringify({
                baseUrl: "https://a",
                token: "",
            }),
        );

        expect(readStoredConnection(storage)).toBeNull();

        storage.setItem(
            "janusgate-web:connection",
            JSON.stringify([
                "https://a",
                "token",
            ]),
        );

        expect(readStoredConnection(storage)).toBeNull();
    });
});
