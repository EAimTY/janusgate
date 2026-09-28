import { render } from "@solidjs/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import { connectionService } from "/src/connection/app-service";
import { ConnectionScreen } from "/src/screens/connection-screen";
import { buttonWithText, labeledInput, settle, typeInto } from "./screen-test-support";

/**
 * The connection screen runs against the application's real connection
 * service: the suite stubs `fetch` so the probe answers from fixtures, and
 * drives the mid-session 401 by reporting it the way the management client
 * does (README §6.4). Rendering is verified against a live gateway in a
 * browser; these tests pin what a reconnect must restore, and the copy
 * affordance the origin requirement puts before that first connect.
 */
function jsonResponse(status: number, payload: unknown): Promise<Response> {
    return Promise.resolve(
        new Response(JSON.stringify(payload), {
            status,
            headers: {
                "Content-Type": "application/json",
            },
        }),
    );
}

/**
 * Installs a `fetch` stub simulating a probe that hangs until its abort
 * signal fires, then rejects with the standard `AbortError`: only the
 * caller's abort can end it, never an answer from the gateway.
 */
function stubAbortingFetch(): void {
    vi.stubGlobal(
        "fetch",
        (_url: unknown, init?: RequestInit) =>
            new Promise<never>((_resolve, reject) => {
                init?.signal?.addEventListener("abort", () => {
                    reject(new DOMException("This operation was aborted", "AbortError"));
                });
            }),
    );
}

function submitConnectionForm(): void {
    const form = document.querySelector("form");

    if (form === null) {
        throw new Error("No connection form");
    }

    form.dispatchEvent(
        new Event("submit", {
            bubbles: true,
            cancelable: true,
        }),
    );
}

describe("ConnectionScreen", () => {
    const gatewayAddress = "https://gateway.example.com";
    let dispose: (() => void) | undefined;

    afterEach(() => {
        dispose?.();
        dispose = undefined;
        vi.unstubAllGlobals();
        // The environment may offer no storage at all; the service then lives
        // in memory for the file, which needs no cleanup beyond disposal.
        window.localStorage?.clear();
        // The service singleton outlives one test, and one forget cannot reset
        // it: from connected — the very rotation this suite tests — it
        // dispatches the forgotten address as the seed. The second forget runs
        // from that unconnected state, captures nothing, and dispatches
        // `baseUrl: null`, so after both the state equals the initial one:
        // unconnected, no failure, no forgotten seed, whatever the test left.
        connectionService.forget();
        connectionService.forget();
    });

    it("offers this app's origin as a copyable value in the requirements", async () => {
        dispose = render(() => <ConnectionScreen />, document.body);
        await settle();

        // The origin is the one requirement value the operator must transfer
        // verbatim into the gateway's configuration file, often elsewhere —
        // it carries the shared copy affordance, naming this page's origin.
        expect(
            document.querySelector('button[aria-label="Copy this app\'s origin"]'),
        ).not.toBeNull();
        expect(document.body.textContent).toContain(window.location.origin);
    });

    it("reseeds the known address and leaves the token empty for re-entry", async () => {
        // The session is already connected, so the screen has never been
        // filled: the field is empty while the app administers the gateway.
        vi.stubGlobal("fetch", () =>
            jsonResponse(200, {
                providers: [],
            }),
        );
        dispose = render(() => <ConnectionScreen />, document.body);
        await settle();

        await connectionService.connect({
            baseUrl: gatewayAddress,
            token: "dying token",
        });
        await settle();

        expect(labeledInput("Gateway address").value).toBe("");
        expect(labeledInput("Management token").value).toBe("");

        // The mid-session 401: the pair drops and the screen explains it,
        // carrying the address the app already knew — never the token, which
        // only the user can re-enter.
        vi.stubGlobal("fetch", () =>
            jsonResponse(401, {
                errno: 2,
                message: "invalid management token",
            }),
        );
        connectionService.reportUnauthorized("invalid management token");
        await settle();

        expect(labeledInput("Gateway address").value).toBe(gatewayAddress);
        expect(labeledInput("Management token").value).toBe("");
        expect(document.body.textContent).toContain("invalid management token");

        // A refused re-probe leaves the field holding exactly the address the
        // attempt addressed: the entered form is normalized into the origin.
        typeInto(labeledInput("Gateway address"), `${gatewayAddress}///`);
        typeInto(labeledInput("Management token"), "replacement token");
        await settle();
        submitConnectionForm();
        await settle();

        expect(labeledInput("Gateway address").value).toBe(gatewayAddress);
        expect(connectionService.currentConnection()).toBeNull();
    });

    it("reopens the forgotten connection's address and leaves the token empty", async () => {
        // The session is connected and the screen has never been filled: both
        // fields are empty while the app administers the gateway.
        vi.stubGlobal("fetch", () =>
            jsonResponse(200, {
                providers: [],
            }),
        );
        dispose = render(() => <ConnectionScreen />, document.body);
        await settle();

        await connectionService.connect({
            baseUrl: gatewayAddress,
            token: "dying token",
        });
        await settle();

        // The standard manual token rotation: Disconnect, then the armed
        // confirm's Disconnect. The forgotten connection's address reopens in the field —
        // the origin need not be retyped — and since nothing was refused, no
        // diagnosis stands. The token structurally cannot travel.
        connectionService.forget();
        await settle();

        expect(labeledInput("Gateway address").value).toBe(gatewayAddress);
        expect(labeledInput("Management token").value).toBe("");
        expect(document.querySelector("[role='alert']")).toBeNull();
    });

    it("raises both fields' errors on a refused submit and retracts each on fix", async () => {
        const probe = vi.fn(() =>
            jsonResponse(200, {
                providers: [],
            }),
        );
        vi.stubGlobal("fetch", probe);

        dispose = render(() => <ConnectionScreen />, document.body);
        await settle();

        // A pristine form states no error — the hints carry the requirements
        // quietly — so the first submit opens the fields' errors and holds
        // the probe back.
        submitConnectionForm();
        await settle();

        expect(document.body.textContent).toContain("Enter the JanusGate base URL.");
        expect(document.body.textContent).toContain("Enter the management token.");
        expect(probe).not.toHaveBeenCalled();

        // Each error derives from its field's current value: fixing one
        // retracts it without another submit, while the untouched field's
        // stands.
        typeInto(labeledInput("Gateway address"), gatewayAddress);
        await settle();

        expect(document.body.textContent).not.toContain("Enter the JanusGate base URL.");
        expect(document.body.textContent).toContain("Enter the management token.");

        typeInto(labeledInput("Management token"), "replacement token");
        await settle();

        expect(document.body.textContent).not.toContain("Enter the management token.");

        // The corrected resubmit probes and connects exactly as a first-valid
        // submit would.
        submitConnectionForm();
        await settle();

        expect(probe).toHaveBeenCalledTimes(1);
        expect(connectionService.currentConnection()).toEqual({
            baseUrl: gatewayAddress,
            token: "replacement token",
        });
    });

    it("returns the form pristine when the operator cancels a probing submit", async () => {
        // The probe hangs until its abort signal fires, so only the Cancel
        // control can end it: the assertions pin that the control aborts the
        // in-flight request and that the interrupted form reopens pristine —
        // no seeded address, no failure notice — rather than as a refusal.
        stubAbortingFetch();

        dispose = render(() => <ConnectionScreen />, document.body);
        await settle();

        typeInto(labeledInput("Gateway address"), gatewayAddress);
        typeInto(labeledInput("Management token"), "any token");
        await settle();
        submitConnectionForm();
        await settle();

        // While probing, the form holds the candidate and offers the escape.
        expect(labeledInput("Gateway address").value).toBe(gatewayAddress);
        expect(labeledInput("Management token").value).toBe("any token");
        expect(labeledInput("Gateway address").disabled).toBe(true);
        buttonWithText("Cancel");

        buttonWithText("Cancel").click();
        await settle();

        // Pristine: both fields empty and enabled, no notice anywhere, and
        // the submit button back to its plain Connect label — alone, since
        // the Cancel control exists only while a probe is in flight.
        expect(labeledInput("Gateway address").value).toBe("");
        expect(labeledInput("Management token").value).toBe("");
        expect(labeledInput("Gateway address").disabled).toBe(false);
        expect(labeledInput("Management token").disabled).toBe(false);
        expect(document.querySelector("[role='alert']")).toBeNull();
        expect(document.body.textContent).not.toContain("cancelled");
        expect(
            [
                ...document.querySelectorAll("button"),
            ]
                .map((button) => button.textContent ?? "")
                .filter((label) => label.trim() !== ""),
        ).toEqual([
            "Copy",
            "Connect",
        ]);
    });

    it("seeds a later refused submit normally after a cancellation", async () => {
        // The cancellation bypasses the seeding for its own landing only: a
        // genuine refusal after it seeds the address exactly as any refusal
        // does.
        stubAbortingFetch();

        dispose = render(() => <ConnectionScreen />, document.body);
        await settle();

        typeInto(labeledInput("Gateway address"), gatewayAddress);
        typeInto(labeledInput("Management token"), "any token");
        await settle();
        submitConnectionForm();
        await settle();
        buttonWithText("Cancel").click();
        await settle();

        expect(labeledInput("Gateway address").value).toBe("");

        // The retry is a genuine refusal: the gateway rejects the token, and
        // the field reopens seeded with the address the attempt carried.
        vi.stubGlobal("fetch", () =>
            jsonResponse(401, {
                errno: 2,
                message: "invalid management token",
            }),
        );
        typeInto(labeledInput("Gateway address"), gatewayAddress);
        typeInto(labeledInput("Management token"), "replacement token");
        await settle();
        submitConnectionForm();
        await settle();

        expect(labeledInput("Gateway address").value).toBe(gatewayAddress);
        expect(document.body.textContent).toContain("invalid management token");
        expect(connectionService.currentConnection()).toBeNull();
    });

    it("keeps the seeded address when the operator cancels a probing resubmit", async () => {
        // The mid-session 401 seeds the address precisely so the reconnect
        // never demands an origin retype — and cancelling the hanging
        // resubmit is no reason to lose it: the token, the errors, and the
        // diagnosis clear, while the seeded address stays.
        vi.stubGlobal("fetch", () =>
            jsonResponse(200, {
                providers: [],
            }),
        );
        dispose = render(() => <ConnectionScreen />, document.body);
        await settle();

        await connectionService.connect({
            baseUrl: gatewayAddress,
            token: "dying token",
        });
        await settle();
        vi.stubGlobal("fetch", () =>
            jsonResponse(401, {
                errno: 2,
                message: "invalid management token",
            }),
        );
        connectionService.reportUnauthorized("invalid management token");
        await settle();

        expect(labeledInput("Gateway address").value).toBe(gatewayAddress);

        // The resubmit hangs until its abort signal fires, so only the
        // Cancel control can end it.
        stubAbortingFetch();
        typeInto(labeledInput("Management token"), "any token");
        await settle();
        submitConnectionForm();
        await settle();
        buttonWithText("Cancel").click();
        await settle();

        expect(labeledInput("Gateway address").value).toBe(gatewayAddress);
        expect(labeledInput("Management token").value).toBe("");
        expect(labeledInput("Management token").disabled).toBe(false);
        expect(document.querySelector("[role='alert']")).toBeNull();
        expect(document.body.textContent).not.toContain("invalid management token");
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

            // The connect probe refuses the way a blocked fetch does, so the
            // failure lands as "no answer" and the reachability probe issues.
            if (String(url).endsWith("/providers:list")) {
                return Promise.reject(new TypeError("Failed to fetch"));
            }

            return new Promise<never>(() => {});
        });

        dispose = render(() => <ConnectionScreen />, document.body);
        await settle();

        typeInto(labeledInput("Gateway address"), gatewayAddress);
        typeInto(labeledInput("Management token"), "any token");
        await settle();
        submitConnectionForm();
        await settle();

        const reachability = requests.find((request) => request.url === `${gatewayAddress}/`);

        expect(reachability).toBeDefined();
        expect(reachability?.init.signal).toBeInstanceOf(AbortSignal);
        expect(document.body.textContent).toContain(`No answer from ${gatewayAddress}.`);
    });

    it("aborts the reachability refinement when the screen goes away", async () => {
        // The refinement probe is owned by the diagnosis that issued it: when
        // the screen goes away the probe is aborted like every in-flight call
        // (README §6.4), never left running toward an address that may never
        // answer.
        vi.stubGlobal("location", {
            ...window.location,
            origin: "https://admin.example.com",
        });

        const reachabilityRequests: Array<{
            signal: AbortSignal | null | undefined;
        }> = [];
        vi.stubGlobal("fetch", (url: string | URL, init?: RequestInit) => {
            // The connect probe refuses the way a blocked fetch does, so the
            // failure lands as "no answer" and the reachability probe issues.
            if (String(url).endsWith("/providers:list")) {
                return Promise.reject(new TypeError("Failed to fetch"));
            }

            reachabilityRequests.push({
                signal: init?.signal,
            });
            return new Promise<never>(() => {});
        });

        dispose = render(() => <ConnectionScreen />, document.body);
        await settle();

        typeInto(labeledInput("Gateway address"), gatewayAddress);
        typeInto(labeledInput("Management token"), "any token");
        await settle();
        submitConnectionForm();
        await settle();

        const reachability = reachabilityRequests.at(0);

        expect(reachability?.signal).toBeInstanceOf(AbortSignal);
        expect(reachability?.signal?.aborted).toBe(false);

        dispose?.();
        dispose = undefined;

        expect(reachability?.signal?.aborted).toBe(true);
    });

    it("diagnoses an HTTPS page's HTTP gateway address without issuing the doomed probe", async () => {
        vi.stubGlobal("location", {
            ...window.location,
            origin: "https://admin.example.com",
        });
        const refusedFetch = vi.fn(() => Promise.reject(new TypeError("Failed to fetch")));
        vi.stubGlobal("fetch", refusedFetch);

        dispose = render(() => <ConnectionScreen />, document.body);
        await settle();

        typeInto(labeledInput("Gateway address"), "http://gateway.example.com");
        typeInto(labeledInput("Management token"), "any token");
        await settle();
        submitConnectionForm();
        await settle();

        // An HTTPS page cannot call an HTTP address: the browser refuses the
        // call deterministically before any request is sent, so the diagnosis
        // is certain on arrival and the reachability probe is never issued —
        // the refused connect call is the only request.
        expect(refusedFetch).toHaveBeenCalledTimes(1);
        expect(document.body.textContent).toContain(
            "An HTTPS page cannot call the HTTP address http://gateway.example.com.",
        );
        expect(document.body.textContent).toContain("Serve this app over HTTP as well");
        expect(document.body.textContent).toContain("`tls`");
    });
});
