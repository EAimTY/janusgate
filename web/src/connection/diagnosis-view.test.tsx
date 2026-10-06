import { createRoot } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Diagnosis } from "/src/management-api/diagnosis";
import { ManagementApiError } from "/src/management-api/errors";
import { settle } from "/src/test-support/management-api";
import { createConnectionDiagnosis } from "./diagnosis-view";
import { createConnectionService } from "./service";

/**
 * The diagnosis machine runs against its own connection service: the probe
 * is injected, so a refusal is staged without the network, and the
 * reachability refinement's request is observed through a `fetch` stub. The
 * screen that mounts this machine is verified in its own suite; these tests
 * pin the machine itself — what a refusal diagnoses, and what its refinement
 * owns.
 */
const gatewayAddress = "https://gateway.example.com";

type Harness = {
    readonly diagnosis: () => Diagnosis | null;
    readonly connect: (baseUrl: string) => Promise<void>;
    readonly dispose: () => void;
};

/**
 * Mounts the machine on a fresh service whose probe refuses the way the real
 * one does when nothing answers: the transport failure the management client
 * raises for a fetch the browser refused — no HTTP status, the `TypeError` as
 * its cause — so the connect attempt lands as "no answer" and the reachability
 * refinement issues.
 */
function mountDiagnosis(): Harness {
    let diagnosis: (() => Diagnosis | null) | undefined;
    const service = createConnectionService({
        storage: null,
        probe: () =>
            Promise.reject(
                new ManagementApiError("transport", "Failed to fetch", {
                    httpStatus: null,
                    cause: new TypeError("Failed to fetch"),
                }),
            ),
    });
    const dispose = createRoot((teardown) => {
        diagnosis = createConnectionDiagnosis({
            state: service.connectionState,
            fieldBaseUrl: () => "",
            onSeed: () => {},
        });
        return teardown;
    });

    if (diagnosis === undefined) {
        throw new Error("The diagnosis machine did not mount.");
    }

    return {
        diagnosis,
        connect: (baseUrl) =>
            service.connect({
                baseUrl,
                token: "any token",
            }),
        dispose,
    };
}

describe("createConnectionDiagnosis", () => {
    afterEach(() => {
        vi.unstubAllGlobals();
    });

    it("carries the probe deadline into the reachability request", async () => {
        // The no-answer refinement probes the origin on its own, and that
        // request must carry the connection probe's deadline like the connect
        // call does, or the sharper "nothing answered" wording could arrive
        // minutes after the screen gave up. The wiring is pinned here, not
        // the wall clock.
        vi.stubGlobal("location", {
            ...window.location,
            origin: "https://admin.example.com",
        });

        const requests: Array<{
            url: string;
            init: RequestInit;
        }> = [];
        vi.stubGlobal("fetch", (url: string | URL, init?: RequestInit) => {
            requests.push({
                url: String(url),
                init: init ?? {},
            });

            return new Promise<never>(() => {});
        });

        const harness = mountDiagnosis();
        await harness.connect(gatewayAddress);
        await settle();

        const reachability = requests.find((request) => request.url === `${gatewayAddress}/`);

        expect(reachability).toBeDefined();
        expect(reachability?.init.signal).toBeInstanceOf(AbortSignal);
        expect(harness.diagnosis()?.kind).toBe("no-answer");
        expect(harness.diagnosis()?.detail).toContain(
            "The browser received no answer it could read.",
        );

        harness.dispose();
    });

    it("aborts the reachability refinement when the owner goes away", async () => {
        // The refinement probe is owned by the diagnosis that issued it: when
        // the machine's owner goes away the probe is aborted like every
        // in-flight call (README §6.4), never left running toward an address
        // that may never answer.
        vi.stubGlobal("location", {
            ...window.location,
            origin: "https://admin.example.com",
        });

        const reachabilityRequests: Array<{
            signal: AbortSignal | null | undefined;
        }> = [];
        vi.stubGlobal("fetch", (_url: string | URL, init?: RequestInit) => {
            reachabilityRequests.push({
                signal: init?.signal,
            });
            return new Promise<never>(() => {});
        });

        const harness = mountDiagnosis();
        await harness.connect(gatewayAddress);
        await settle();

        const reachability = reachabilityRequests.at(0);

        expect(reachability?.signal).toBeInstanceOf(AbortSignal);
        expect(reachability?.signal?.aborted).toBe(false);

        harness.dispose();

        expect(reachability?.signal?.aborted).toBe(true);
    });

    it("diagnoses an HTTPS page's HTTP gateway address without issuing the doomed probe", async () => {
        vi.stubGlobal("location", {
            ...window.location,
            origin: "https://admin.example.com",
        });
        const refusedFetch = vi.fn(() => Promise.reject(new TypeError("Failed to fetch")));
        vi.stubGlobal("fetch", refusedFetch);

        // The gateway address is HTTP while this page is HTTPS: the pairing
        // the browser refuses outright, named by the connect attempt itself.
        const harness = mountDiagnosis();
        await harness.connect("http://gateway.example.com");
        await settle();

        // The browser refuses the call deterministically before any request
        // is sent, so the diagnosis is certain on arrival and the
        // reachability probe is never issued.
        expect(harness.diagnosis()?.kind).toBe("mixed-content");
        expect(harness.diagnosis()?.detail).toContain(
            "This page is served over HTTPS, and the browser blocks its calls to HTTP addresses outright, so the gateway was never contacted.",
        );
        expect(harness.diagnosis()?.fix).not.toBeNull();
        expect(refusedFetch).not.toHaveBeenCalled();

        harness.dispose();
    });
});
