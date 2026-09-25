import { render } from "@solidjs/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import { connectionService } from "/src/connection/app-service";
import { ConnectionScreen } from "/src/screens/connection-screen";
import { settle } from "./screen-test-support";

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

/** The input the given field label names, through the label's `for`. */
function inputForLabelText(labelText: string): HTMLInputElement {
    for (const label of document.querySelectorAll("label")) {
        if (label.textContent === labelText) {
            const input = document.getElementById(label.htmlFor);

            if (input instanceof HTMLInputElement) {
                return input;
            }
        }
    }

    throw new Error(`No input labeled ${labelText}`);
}

/** Types into a field the way a browser delivers keystrokes. */
function typeInto(input: HTMLInputElement, value: string): void {
    input.value = value;
    input.dispatchEvent(
        new Event("input", {
            bubbles: true,
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

        expect(inputForLabelText("Gateway address").value).toBe("");
        expect(inputForLabelText("Management token").value).toBe("");

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

        expect(inputForLabelText("Gateway address").value).toBe(gatewayAddress);
        expect(inputForLabelText("Management token").value).toBe("");
        expect(document.body.textContent).toContain("invalid management token");

        // A refused re-probe leaves the field holding exactly the address the
        // attempt addressed: the entered form is normalized into the origin.
        typeInto(inputForLabelText("Gateway address"), `${gatewayAddress}///`);
        typeInto(inputForLabelText("Management token"), "replacement token");
        await settle();
        submitConnectionForm();
        await settle();

        expect(inputForLabelText("Gateway address").value).toBe(gatewayAddress);
        expect(connectionService.currentConnection()).toBeNull();
    });
});
