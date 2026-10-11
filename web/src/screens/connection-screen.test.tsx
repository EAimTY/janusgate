import type { JSX } from "@solidjs/web";
import { render } from "@solidjs/web";
import { flush } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ConnectionState } from "/src/connection/state";
import { ManagementApiError } from "/src/management-api/errors";
import { ErrorCode } from "/src/management-api/generated/v0";
import { ConnectionScreen } from "/src/screens/connection-screen";

/**
 * The connection service is the network boundary — stubbed whole, so the
 * screen's form contract is tested against a fixture state the test drives.
 */
const connectionHarness = vi.hoisted(() => {
    return {
        setState: undefined as unknown as (next: ConnectionState) => void,
        connectCalls: [] as Array<{
            readonly baseUrl: string;
            readonly token: string;
        }>,
        cancelCount: 0,
    };
});

vi.mock("/src/connection/app-service", async () => {
    const { createSignal } = await import("solid-js");
    const { initialConnectionState } = await import("/src/connection/state");
    const [state, setState] = createSignal<ConnectionState>(initialConnectionState);
    connectionHarness.setState = setState;

    return {
        connectionService: {
            connectionState: state,
            connect: (input: { baseUrl: string; token: string }) => {
                connectionHarness.connectCalls.push(input);
                return Promise.resolve();
            },
        },
        cancelConnectionProbe: () => {
            connectionHarness.cancelCount += 1;
            // The machine's cancellation: setup reopens carrying no failure.
            connectionHarness.setState?.({
                status: "unconnected",
                failure: null,
                forgottenBaseUrl: null,
            });
        },
    };
});

const pristine: ConnectionState = {
    status: "unconnected",
    failure: null,
    forgottenBaseUrl: null,
};

let disposeView: (() => void) | null = null;

function mount(view: () => JSX.Element): HTMLElement {
    const host = document.createElement("div");
    document.body.appendChild(host);
    disposeView = render(view, host);
    return host;
}

function requireElement<TElement extends Element>(host: ParentNode, selectors: string): TElement {
    const element = host.querySelector<TElement>(selectors);
    if (element === null) {
        throw new Error(`Missing element for "${selectors}".`);
    }

    return element;
}

function inputForLabel(host: ParentNode, labelText: string): HTMLInputElement {
    const label = Array.from(host.querySelectorAll("label")).find(
        (candidate) => candidate.textContent?.trim() === labelText,
    );
    const inputId = label?.getAttribute("for") ?? null;
    const input =
        inputId === null ? null : host.querySelector<HTMLInputElement>(`input[id='${inputId}']`);
    if (input === null) {
        throw new Error(`No input labeled "${labelText}".`);
    }

    return input;
}

function typeInto(element: HTMLInputElement, value: string): void {
    element.value = value;
    element.dispatchEvent(
        new Event("input", {
            bubbles: true,
        }),
    );
    flush();
}

function submitForm(host: HTMLElement): void {
    const form = requireElement<HTMLFormElement>(host, "form");
    form.dispatchEvent(
        new Event("submit", {
            bubbles: true,
            cancelable: true,
        }),
    );
    flush();
}

function buttonByLabel(host: ParentNode, label: string): HTMLButtonElement {
    const button = Array.from(host.querySelectorAll("button")).find((candidate) =>
        candidate.textContent?.trim().startsWith(label),
    );
    if (button === undefined) {
        throw new Error(`No button labeled "${label}".`);
    }

    return button;
}

beforeEach(() => {
    connectionHarness.connectCalls.length = 0;
    connectionHarness.cancelCount = 0;
    connectionHarness.setState?.(pristine);
});

afterEach(() => {
    disposeView?.();
    disposeView = null;
    document.body.replaceChildren();
    connectionHarness.setState?.(pristine);
});

describe("ConnectionScreen", () => {
    it("opens masked, and the reveal lifts the mask while the operator types", () => {
        const host = mount(() => <ConnectionScreen />);

        const token = inputForLabel(host, "Management token");
        expect(token.type).toBe("password");

        const reveal = requireElement<HTMLInputElement>(host, "input[type='checkbox']");
        expect(reveal.checked).toBe(false);
        reveal.click();
        flush();
        expect(token.type).toBe("text");
    });

    it("opens the fields' errors behind the first submit, then follows each field's value", () => {
        const host = mount(() => <ConnectionScreen />);

        // A pristine form is not an error; the hints state the requirements.
        expect(host.querySelector("p[role='alert']")).toBeNull();

        submitForm(host);
        expect(host.querySelectorAll("p[role='alert']").length).toBe(2);
        expect(host.textContent).toContain("Enter the JanusGate base URL.");
        expect(host.textContent).toContain("Enter the management token.");
        expect(connectionHarness.connectCalls.length).toBe(0);

        // Fixing one field retracts its error without another submit.
        typeInto(inputForLabel(host, "Gateway address"), "https://gateway.example.com");
        expect(host.textContent).not.toContain("Enter the JanusGate base URL.");
        expect(host.textContent).toContain("Enter the management token.");
    });

    it("connects with the normalized address and the token as typed", () => {
        const host = mount(() => <ConnectionScreen />);

        typeInto(inputForLabel(host, "Gateway address"), "  https://gateway.example.com/  ");
        typeInto(inputForLabel(host, "Management token"), "tok_123");
        submitForm(host);

        expect(connectionHarness.connectCalls).toEqual([
            {
                baseUrl: "https://gateway.example.com",
                token: "tok_123",
            },
        ]);
    });

    it("refuses a scheme the browser cannot call and stays put", () => {
        const host = mount(() => <ConnectionScreen />);

        typeInto(inputForLabel(host, "Gateway address"), "ftp://gateway.example.com");
        typeInto(inputForLabel(host, "Management token"), "tok_123");
        submitForm(host);

        expect(host.textContent).toContain("Use an http:// or https:// URL.");
        expect(connectionHarness.connectCalls.length).toBe(0);
    });

    it("stands down while the probe runs, and a cancel ends the attempt without a diagnosis", () => {
        connectionHarness.setState?.({
            status: "verifying",
        });
        const host = mount(() => <ConnectionScreen />);

        const address = inputForLabel(host, "Gateway address");
        typeInto(address, "https://gateway.example.com");
        typeInto(inputForLabel(host, "Management token"), "tok_123");

        expect(buttonByLabel(host, "Probing the gateway…").disabled).toBe(true);
        expect(address.disabled).toBe(true);

        // The attempt in flight is abandoned: the fields close out, and the
        // machine's cancellation lands — no failure, nothing to diagnose.
        const tokenBefore = inputForLabel(host, "Management token").value;
        expect(tokenBefore).toBe("tok_123");
        buttonByLabel(host, "Cancel").click();
        flush();

        expect(connectionHarness.cancelCount).toBe(1);
        expect(inputForLabel(host, "Management token").value).toBe("");
        expect(inputForLabel(host, "Gateway address").value).toBe("");
        expect(host.querySelector("p[role='alert']")).toBeNull();
    });

    it("seeds the address from a refused attempt and keeps the gateway's message verbatim", async () => {
        const refused: ConnectionState = {
            status: "unconnected",
            failure: {
                message: "Invalid management token.",
                cause: new ManagementApiError("gateway", "Invalid management token.", {
                    errno: ErrorCode.Unauthorized,
                    httpStatus: 401,
                    baseUrl: "https://gateway.example.com",
                }),
                baseUrl: "https://gateway.example.com",
            },
            forgottenBaseUrl: null,
        };
        connectionHarness.setState?.(refused);
        const host = mount(() => <ConnectionScreen />);

        await vi.waitFor(() => {
            flush();
            expect(host.textContent).toContain("The gateway rejected that management token.");
        });
        // The gateway's message is the detail, verbatim.
        expect(host.textContent).toContain("Invalid management token.");
        // The refusal's address seeds the field, so a reconnect never
        // demands an origin retype.
        expect(inputForLabel(host, "Gateway address").value).toBe("https://gateway.example.com");
    });
});
