import { createRoot, createSignal } from "solid-js";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { ManagementApiError } from "/src/management-api/errors";
import { createConnectionDiagnosis } from "./diagnosis-view";
import type { ConnectionState } from "./state";

/**
 * The setup screen's reactive diagnosis machine: a landing without a failure
 * diagnoses nothing, a refusal is diagnosed from the address its failure
 * carried, the form is seeded with whatever address the landing carries, and
 * a "no answer" verdict is refined by one reachability probe that a newer
 * landing or the screen's disposal aborts.
 */

beforeEach(() => {
    vi.stubGlobal(
        "fetch",
        vi.fn(async () => {
            throw new TypeError("fetch failed");
        }),
    );
});

afterEach(() => {
    vi.unstubAllGlobals();
});

/** Advances the microtask queue far enough for effects and probe callbacks to settle. */
async function settle(): Promise<void> {
    for (let round = 0; round < 6; round += 1) {
        await new Promise<void>((resolve) => setTimeout(resolve, 0));
    }
}

const UNCONNECTED: ConnectionState = {
    status: "unconnected",
    failure: null,
    forgottenBaseUrl: null,
};

const REFUSED: ConnectionState = {
    status: "unconnected",
    failure: {
        message: "Network request failed: fetch failed",
        // The browser refused the call before any answer existed — the
        // transport shape whose diagnosis opens with the no-answer verdict
        // the probe may refine.
        cause: new ManagementApiError("transport", "Network request failed: fetch failed"),
        baseUrl: "https://gateway.example.test:8787",
    },
    forgottenBaseUrl: null,
};

describe("createConnectionDiagnosis", () => {
    test("diagnoses nothing for a landing without a failure", async () => {
        await createRoot(async (dispose) => {
            const [state] = createSignal<ConnectionState>(UNCONNECTED);
            const seeds: Array<string | null> = [];
            const diagnosis = createConnectionDiagnosis({
                state,
                fieldBaseUrl: () => "",
                onSeed: (baseUrl) => seeds.push(baseUrl),
            });
            await settle();

            expect(diagnosis()).toBeNull();
            expect(seeds).toEqual([
                null,
            ]);
            dispose();
        });
    });

    test("diagnoses a refusal and seeds the form with its attempted address", async () => {
        await createRoot(async (dispose) => {
            const [state, setState] = createSignal<ConnectionState>(UNCONNECTED);
            const seeds: Array<string | null> = [];
            const diagnosis = createConnectionDiagnosis({
                state,
                fieldBaseUrl: () => "",
                onSeed: (baseUrl) => seeds.push(baseUrl),
            });

            await settle();
            setState(REFUSED);
            await settle();

            const current = diagnosis();
            expect(current).not.toBeNull();
            if (current === null) throw new Error("expected a diagnosis");
            expect(current.context.baseUrl).toBe("https://gateway.example.test:8787");
            expect(seeds.at(-1)).toBe("https://gateway.example.test:8787");
            dispose();
        });
    });

    test("seeds a forget's address and clears the diagnosis of the landing before it", async () => {
        await createRoot(async (dispose) => {
            const [state, setState] = createSignal<ConnectionState>(REFUSED);
            const seeds: Array<string | null> = [];
            const diagnosis = createConnectionDiagnosis({
                state,
                fieldBaseUrl: () => "",
                onSeed: (baseUrl) => seeds.push(baseUrl),
            });

            await settle();
            setState({
                status: "unconnected",
                failure: null,
                forgottenBaseUrl: "http://forgotten.test:9000",
            });
            await settle();

            expect(diagnosis()).toBeNull();
            expect(seeds.at(-1)).toBe("http://forgotten.test:9000");
            dispose();
        });
    });

    test("falls back to the form field's address when the failure carries none", async () => {
        await createRoot(async (dispose) => {
            const [state, setState] = createSignal<ConnectionState>(UNCONNECTED);
            const diagnosis = createConnectionDiagnosis({
                state,
                fieldBaseUrl: () => "http://typed.test:8787",
                onSeed: () => {},
            });

            await settle();
            setState({
                status: "unconnected",
                failure: {
                    message: "Network request failed",
                    cause: new Error("x"),
                    baseUrl: null,
                },
                forgottenBaseUrl: null,
            });
            await settle();

            const current = diagnosis();
            if (current === null) throw new Error("expected a diagnosis");
            expect(current.context.baseUrl).toBe("http://typed.test:8787");
            dispose();
        });
    });

    test("refines a no-answer verdict through the reachability probe", async () => {
        vi.stubGlobal(
            "fetch",
            vi.fn(
                async () =>
                    new Response(null, {
                        status: 200,
                    }),
            ),
        );

        await createRoot(async (dispose) => {
            const [state, setState] = createSignal<ConnectionState>(UNCONNECTED);
            const diagnosis = createConnectionDiagnosis({
                state,
                fieldBaseUrl: () => "",
                onSeed: () => {},
            });

            // An https target, so the verdict is the open no-answer one.
            await settle();
            setState(REFUSED);
            await settle();

            const current = diagnosis();
            if (current === null) throw new Error("expected a diagnosis");
            // An answered probe proves the network path: the block is the browser's CORS layer.
            expect(current.kind).toBe("blocked");
            dispose();
        });
    });

    test("a newer landing supersedes a refinement still in flight", async () => {
        // A probe that never answers: the refinement stays pending until superseded.
        vi.stubGlobal(
            "fetch",
            vi.fn(
                () =>
                    new Promise<Response>(() => {
                        // Never settles.
                    }),
            ),
        );

        await createRoot(async (dispose) => {
            const [state, setState] = createSignal<ConnectionState>(UNCONNECTED);
            const diagnosis = createConnectionDiagnosis({
                state,
                fieldBaseUrl: () => "",
                onSeed: () => {},
            });

            await settle();
            setState(REFUSED);
            await settle();

            // A cancellation landing replaces the refusal: control flow, no write-up.
            setState(UNCONNECTED);
            await settle();

            expect(diagnosis()).toBeNull();
            dispose();
        });
    });
});
