import { flush } from "solid-js";
import { describe, expect, it } from "vitest";
import { appConfig } from "/src/config";
import { ManagementApiError } from "/src/management-api/errors";
import type { Connection } from "./model";
import { readStoredConnection } from "./model";
import { createConnectionService } from "./service";

const connection: Connection = {
    baseUrl: "https://gateway.example.com",
    token: "secret",
};

function createStorage(seed?: Connection): Storage {
    const entries = new Map<string, string>();

    if (seed !== undefined) {
        entries.set(appConfig.connectionStorageKey, JSON.stringify(seed));
    }

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
}

describe("createConnectionService", () => {
    it("resumes a stored connection", () => {
        const service = createConnectionService({
            storage: createStorage(connection),
            probe: async () => {
                throw new Error("must not probe on startup");
            },
        });

        expect(service.connectionState()).toEqual({
            status: "connected",
            connection,
        });

        expect(service.currentConnection()).toEqual(connection);
    });

    it("starts unconnected without storage", () => {
        const service = createConnectionService({
            storage: null,
            probe: async () => {},
        });

        expect(service.connectionState()).toEqual({
            status: "unconnected",
            failure: null,
            forgottenBaseUrl: null,
        });

        expect(service.currentConnection()).toBeNull();
    });

    it("verifies, persists and accepts a candidate", async () => {
        const storage = createStorage();
        const probed: Connection[] = [];

        const service = createConnectionService({
            storage,
            probe: async (candidate) => {
                probed.push(candidate);
            },
        });

        await service.connect(connection);

        expect(probed).toEqual([
            connection,
        ]);

        expect(readStoredConnection(storage)).toEqual(connection);

        expect(service.connectionState()).toEqual({
            status: "connected",
            connection,
        });
    });

    it("keeps the visitor on setup with the failure message when the probe fails", async () => {
        const storage = createStorage();

        const service = createConnectionService({
            storage,
            probe: async () => {
                throw new Error("Failed to fetch");
            },
        });

        await service.connect(connection);

        expect(readStoredConnection(storage)).toBeNull();

        expect(service.connectionState()).toEqual({
            status: "unconnected",
            failure: expect.objectContaining({
                message: expect.stringContaining("Failed to fetch"),
            }),
            forgottenBaseUrl: null,
        });

        expect(service.currentConnection()).toBeNull();
    });

    it("cancels the probe in flight into a clean setup, never a refusal", async () => {
        const storage = createStorage();
        const signals: Array<AbortSignal> = [];

        const service = createConnectionService({
            storage,
            // The transport hangs until its signal fires, so only the
            // service's cancel channel can end the attempt.
            probe: (_candidate, signal) =>
                new Promise<void>((_resolve, reject) => {
                    signals.push(signal);
                    signal.addEventListener("abort", () => reject(signal.reason));
                }),
        });

        const attempt = service.connect(connection);
        service.cancelConnect();
        await attempt;

        expect(signals).toHaveLength(1);
        expect(signals[0]?.aborted).toBe(true);

        // The cancelled attempt persists nothing and carries no failure: the
        // machine records a cancellation, so setup reopens pristine instead
        // of seeding a refusal no gateway produced.
        expect(readStoredConnection(storage)).toBeNull();

        expect(service.connectionState()).toEqual({
            status: "unconnected",
            failure: null,
            forgottenBaseUrl: null,
        });
    });

    it("lands a transport answer that rides through the cancel as a cancellation", async () => {
        const storage = createStorage();

        const service = createConnectionService({
            storage,
            // A transport that cannot abort may still answer after the
            // cancel: the operator's word stands either way.
            probe: (_candidate, signal) =>
                new Promise<void>((resolve) => {
                    signal.addEventListener("abort", () => setTimeout(resolve, 0));
                }),
        });

        const attempt = service.connect(connection);
        service.cancelConnect();
        await attempt;

        expect(readStoredConnection(storage)).toBeNull();

        expect(service.connectionState()).toEqual({
            status: "unconnected",
            failure: null,
            forgottenBaseUrl: null,
        });
    });

    it("treats a transport abort the service did not order as a refusal", async () => {
        const storage = createStorage();
        // The probe's own deadline aborts against the transport, while the
        // service's cancel channel stays clean: the attempt fails like any
        // refusal instead of reading as a cancellation.
        const timeout = new DOMException("The operation timed out.", "TimeoutError");

        const service = createConnectionService({
            storage,
            probe: async () => {
                throw timeout;
            },
        });

        await service.connect(connection);

        expect(readStoredConnection(storage)).toBeNull();

        expect(service.connectionState()).toEqual({
            status: "unconnected",
            failure: expect.objectContaining({
                // The message carries the diagnosis whatever the environment's
                // rendering of the abort (jsdom's DOMException is not an Error,
                // so the failure's message derives from its string form).
                message: expect.stringContaining("The operation timed out."),
                cause: timeout,
                baseUrl: connection.baseUrl,
            }),
            forgottenBaseUrl: null,
        });
    });

    it("never probes while a connection is being verified or already held", async () => {
        const storage = createStorage();
        let probeCount = 0;
        let releaseProbe: (() => void) | undefined;

        const service = createConnectionService({
            storage,
            probe: async () => {
                probeCount += 1;
                await new Promise<void>((resolve) => {
                    releaseProbe = resolve;
                });
            },
        });

        const firstAttempt = service.connect(connection);

        await service.connect({
            baseUrl: "https://other.example.com",
            token: "other",
        });

        releaseProbe?.();
        await firstAttempt;

        expect(probeCount).toBe(1);

        expect(service.connectionState()).toEqual({
            status: "connected",
            connection,
        });
    });

    it("forgets the stored pair, leaving its address for the setup form", () => {
        const storage = createStorage(connection);

        const service = createConnectionService({
            storage,
            probe: async () => {},
        });

        service.forget();
        flush();

        expect(readStoredConnection(storage)).toBeNull();

        // The forgotten connection's address travels so the setup form
        // reopens with it — the manual token rotation never demands an
        // origin retype; the token structurally cannot travel.
        expect(service.connectionState()).toEqual({
            status: "unconnected",
            failure: null,
            forgottenBaseUrl: "https://gateway.example.com",
        });

        // Forgetting again carries no address: nothing is connected that
        // could be forgotten.
        service.forget();
        flush();

        expect(service.connectionState()).toEqual({
            status: "unconnected",
            failure: null,
            forgottenBaseUrl: null,
        });
    });

    it("clears a rejected connection, keeps its address, and explains it on setup", () => {
        const storage = createStorage(connection);

        const service = createConnectionService({
            storage,
            probe: async () => {},
        });

        service.reportUnauthorized("invalid management token");
        flush();

        expect(readStoredConnection(storage)).toBeNull();

        expect(service.connectionState()).toEqual({
            status: "unconnected",
            failure: {
                message: "invalid management token",
                cause: expect.any(ManagementApiError),
                // The dying connection's address travels so the reconnect
                // screen seeds its field; the token structurally cannot.
                baseUrl: "https://gateway.example.com",
            },
            forgottenBaseUrl: null,
        });
    });
});
