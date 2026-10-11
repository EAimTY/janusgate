import { createRoot } from "solid-js";
import { afterEach, describe, expect, test, vi } from "vitest";
import type { Connection } from "./model";
import { createConnectionService } from "./service";

/**
 * The connection service owning the pair's lifecycle: it resumes what
 * storage holds, probes and persists new candidates, lands an aborted
 * attempt as the machine's own cancellation — never a refusal — captures a
 * forget's and a 401's address before the pair drops, and keeps the
 * in-memory state authoritative when storage refuses. Every interaction
 * runs after a tick, outside the mount's owned scope, the way the app's own
 * event handlers do.
 */

const PAIR: Connection = {
    baseUrl: "http://gw.test:8787",
    token: "t-1",
};

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

/** Resolves the storage the service persists through, seeded with a stored pair when given. */
function storageWithSeed(connection: Connection | null): Storage {
    const storage = memoryStorage();
    if (connection !== null) {
        storage.setItem("janusgate-web:connection", JSON.stringify(connection));
    }
    return storage;
}

/** Advances past the mount's owned scope and the service's synchronous dispatches. */
async function tick(): Promise<void> {
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
}

describe("createConnectionService", () => {
    afterEach(() => {
        vi.restoreAllMocks();
    });

    test("resumes the pair storage holds", async () => {
        await createRoot(async (dispose) => {
            const service = createConnectionService({
                storage: storageWithSeed(PAIR),
                probe: vi.fn(async () => {}),
            });
            await tick();

            expect(service.connectionState()).toEqual({
                status: "connected",
                connection: PAIR,
            });
            expect(service.currentConnection()).toEqual(PAIR);
            dispose();
        });
    });

    test("starts unconnected when storage holds nothing", async () => {
        await createRoot(async (dispose) => {
            const service = createConnectionService({
                storage: storageWithSeed(null),
                probe: vi.fn(async () => {}),
            });
            await tick();

            expect(service.connectionState()).toEqual({
                status: "unconnected",
                failure: null,
                forgottenBaseUrl: null,
            });
            expect(service.currentConnection()).toBeNull();
            dispose();
        });
    });

    test("persists and lands an accepted candidate", async () => {
        const storage = storageWithSeed(null);
        await createRoot(async (dispose) => {
            const service = createConnectionService({
                storage,
                probe: vi.fn(async () => {}),
            });
            await tick();

            await service.connect(PAIR);

            expect(service.connectionState()).toEqual({
                status: "connected",
                connection: PAIR,
            });
            expect(storage.getItem("janusgate-web:connection")).toBe(JSON.stringify(PAIR));
            dispose();
        });
    });

    test("lands a refused probe on setup carrying the attempt's address", async () => {
        const failure = new Error("HTTP 401");
        const storage = storageWithSeed(null);
        await createRoot(async (dispose) => {
            const service = createConnectionService({
                storage,
                probe: vi.fn(async () => {
                    throw failure;
                }),
            });
            await tick();

            await service.connect(PAIR);

            const state = service.connectionState();
            expect(state.status).toBe("unconnected");
            if (state.status !== "unconnected") throw new Error("expected unconnected");
            expect(state.failure?.baseUrl).toBe(PAIR.baseUrl);
            expect(state.failure?.message).toContain("HTTP 401");
            expect(state.failure?.cause).toBe(failure);
            expect(service.currentConnection()).toBeNull();
            expect(storage.getItem("janusgate-web:connection")).toBeNull();
            dispose();
        });
    });

    test("lands the operator's cancel as a cancellation, never a refusal or an acceptance", async () => {
        const signals: AbortSignal[] = [];
        let settleProbe: ((isReject: boolean) => void) | null = null;
        await createRoot(async (dispose) => {
            const storage = storageWithSeed(null);
            const service = createConnectionService({
                storage,
                probe: (_connection, signal) => {
                    signals.push(signal);
                    return new Promise<void>((resolve, reject) => {
                        settleProbe = (isReject: boolean) =>
                            isReject ? reject(new Error("late refusal")) : resolve();
                    });
                },
            });
            await tick();

            const landed = service.connect(PAIR);
            service.cancelConnect();
            expect(signals[0]?.aborted).toBe(true);
            settleProbe?.(false);
            await landed;

            const state = service.connectionState();
            expect(state.status).toBe("unconnected");
            if (state.status !== "unconnected") throw new Error("expected unconnected");
            // Control flow, never a refusal: nothing to diagnose or seed.
            expect(state.failure).toBeNull();
            expect(state.forgottenBaseUrl).toBeNull();
            expect(storage.getItem("janusgate-web:connection")).toBeNull();
            dispose();
        });
    });

    test("lands a cancelled attempt as the cancellation even when the probe answers", async () => {
        let settleProbe: (() => void) | null = null;
        await createRoot(async (dispose) => {
            const storage = storageWithSeed(null);
            const service = createConnectionService({
                storage,
                probe: () =>
                    new Promise<void>((resolve) => {
                        settleProbe = resolve;
                    }),
            });
            await tick();

            const landed = service.connect(PAIR);
            service.cancelConnect();
            // A transport that cannot abort still answers through the cancel.
            settleProbe?.();
            await landed;

            expect(service.connectionState().status).toBe("unconnected");
            expect(storage.getItem("janusgate-web:connection")).toBeNull();
            dispose();
        });
    });

    test("keeps the single-slot gate: a second submission while one runs is refused by the machine", async () => {
        let settleProbe: ((isReject: boolean) => void) | null = null;
        await createRoot(async (dispose) => {
            let probeRuns = 0;
            const service = createConnectionService({
                storage: storageWithSeed(null),
                probe: () => {
                    probeRuns += 1;
                    return new Promise<void>((resolve, reject) => {
                        settleProbe = (isReject: boolean) =>
                            isReject ? reject(new Error("refused")) : resolve();
                    });
                },
            });
            await tick();

            const first = service.connect(PAIR);
            // The connect guard answers the second attempt without a probe.
            await service.connect({
                baseUrl: "http://other.test",
                token: "t-2",
            });
            expect(probeRuns).toBe(1);

            settleProbe?.(false);
            await first;
            dispose();
        });
    });

    test("a connect while connected is a no-op", async () => {
        const probe = vi.fn(async () => {});
        await createRoot(async (dispose) => {
            const service = createConnectionService({
                storage: storageWithSeed(PAIR),
                probe,
            });
            await tick();

            await service.connect({
                baseUrl: "http://other.test",
                token: "t-2",
            });

            expect(service.currentConnection()).toEqual(PAIR);
            expect(probe).not.toHaveBeenCalled();
            dispose();
        });
    });

    test("forgets: storage is cleared and setup reopens seeded with the served address", async () => {
        const storage = storageWithSeed(PAIR);
        await createRoot(async (dispose) => {
            const service = createConnectionService({
                storage,
                probe: vi.fn(async () => {}),
            });
            await tick();

            service.forget();
            // The signal mirror commits with the next flush; the accessor reads settle then.
            await tick();

            const state = service.connectionState();
            expect(state.status).toBe("unconnected");
            if (state.status !== "unconnected") throw new Error("expected unconnected");
            expect(state.failure).toBeNull();
            expect(state.forgottenBaseUrl).toBe(PAIR.baseUrl);
            expect(storage.getItem("janusgate-web:connection")).toBeNull();
            expect(service.currentConnection()).toBeNull();
            dispose();
        });
    });

    test("reports unauthorized: the dying address seeds setup and the cause carries the 401", async () => {
        const storage = storageWithSeed(PAIR);
        await createRoot(async (dispose) => {
            const service = createConnectionService({
                storage,
                probe: vi.fn(async () => {}),
            });
            await tick();

            service.reportUnauthorized("the token is dead");
            await tick();

            const state = service.connectionState();
            expect(state.status).toBe("unconnected");
            if (state.status !== "unconnected") throw new Error("expected unconnected");
            expect(state.failure?.message).toBe("the token is dead");
            expect(state.failure?.baseUrl).toBe(PAIR.baseUrl);
            expect(
                (
                    state.failure?.cause as {
                        errno?: number;
                    }
                )?.errno,
            ).toBe(2);
            expect(storage.getItem("janusgate-web:connection")).toBeNull();
            dispose();
        });
    });

    test("keeps working when storage refuses every operation", async () => {
        const refusing: Storage = {
            ...memoryStorage(),
            getItem: () => {
                throw new Error("denied");
            },
            setItem: () => {
                throw new Error("denied");
            },
            removeItem: () => {
                throw new Error("denied");
            },
        };

        await createRoot(async (dispose) => {
            const service = createConnectionService({
                storage: refusing,
                probe: vi.fn(async () => {}),
            });
            await tick();

            expect(service.connectionState().status).toBe("unconnected");

            await service.connect(PAIR);

            // The in-memory connection remains authoritative for the session.
            expect(service.currentConnection()).toEqual(PAIR);
            service.forget();
            expect(service.currentConnection()).toBeNull();
            dispose();
        });
    });
});
