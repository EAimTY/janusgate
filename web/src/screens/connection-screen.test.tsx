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
