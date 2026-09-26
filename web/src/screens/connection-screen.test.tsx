import { render } from "@solidjs/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import { connectionService } from "/src/connection/app-service";
import { ConnectionScreen } from "/src/screens/connection-screen";
import { labeledInput, settle, typeInto } from "./screen-test-support";

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

        // The standard manual token rotation: Disconnect, then "Forget
        // token". The forgotten connection's address reopens in the field —
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
