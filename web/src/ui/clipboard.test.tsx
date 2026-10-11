import type { JSX } from "@solidjs/web";
import { render } from "@solidjs/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CopyButton } from "/src/ui/clipboard";

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

function stubClipboard(writeText: ((value: string) => Promise<void>) | null): void {
    if (writeText === null) {
        Reflect.deleteProperty(window.navigator, "clipboard");
        return;
    }

    Object.defineProperty(window.navigator, "clipboard", {
        value: {
            writeText,
        },
        configurable: true,
    });
}

function stubExecCommand(returns: boolean): ReturnType<typeof vi.fn<() => boolean>> {
    const execCommand = vi.fn<() => boolean>().mockReturnValue(returns);
    Object.defineProperty(document, "execCommand", {
        value: execCommand,
        configurable: true,
        writable: true,
    });
    return execCommand;
}

function clickCopy(host: HTMLElement): HTMLButtonElement {
    const button = requireElement<HTMLButtonElement>(host, "button");
    button.click();
    return button;
}

async function settle(): Promise<void> {
    await Promise.resolve();
    await Promise.resolve();
}

beforeEach(() => {
    vi.useFakeTimers();
});

afterEach(() => {
    disposeView?.();
    disposeView = null;
    document.body.replaceChildren();
    Reflect.deleteProperty(document, "execCommand");
    stubClipboard(null);
    vi.useRealTimers();
});

describe("CopyButton", () => {
    it("confirms where a value went and reverts the confirmation shortly", async () => {
        const writeText = vi.fn<(value: string) => Promise<void>>().mockResolvedValue(undefined);
        stubClipboard(writeText);
        const host = mount(() => (
            <CopyButton
                value="sk-secret"
                subject="the secret here"
            />
        ));

        const button = clickCopy(host);
        await settle();
        expect(writeText).toHaveBeenCalledWith("sk-secret");
        expect(button.textContent).toContain("Copied");

        await vi.advanceTimersByTimeAsync(1500);
        expect(button.textContent).toContain("Copy");
        expect(button.textContent).not.toContain("Copied");
    });

    it("states a refusal where the browser refuses, and the refusal stands", async () => {
        stubClipboard(
            vi.fn<(value: string) => Promise<void>>().mockRejectedValue(new Error("denied")),
        );
        stubExecCommand(false);
        const host = mount(() => (
            <CopyButton
                value="https://gateway.example/api"
                subject="the URL beside it"
            />
        ));

        clickCopy(host);
        await vi.advanceTimersByTimeAsync(0);

        const refusal = requireElement<HTMLSpanElement>(host, "span[role='status']");
        expect(refusal.textContent).toContain(
            "This browser refused the clipboard — select the URL beside it and copy it by hand.",
        );

        // A refusal diagnoses the environment; it must outlive the click.
        await vi.advanceTimersByTimeAsync(10_000);
        expect(host.querySelector("span[role='status']")).not.toBeNull();
    });

    it("takes the legacy path when no Clipboard API exists", async () => {
        stubClipboard(null);
        const execCommand = stubExecCommand(true);
        const host = mount(() => <CopyButton value="sk-secret" />);

        const button = clickCopy(host);
        await vi.advanceTimersByTimeAsync(0);
        expect(execCommand).toHaveBeenCalledWith("copy");
        expect(button.textContent).toContain("Copied");
        // The transient textarea never survives the attempt.
        expect(document.querySelector("textarea")).toBeNull();
    });

    it("replaces a standing refusal when a later copy is taken", async () => {
        let isRefused = true;
        stubClipboard(
            vi.fn<(value: string) => Promise<void>>(() =>
                isRefused ? Promise.reject(new Error("denied")) : Promise.resolve(),
            ),
        );
        stubExecCommand(false);
        const host = mount(() => <CopyButton value="sk-secret" />);

        clickCopy(host);
        await vi.advanceTimersByTimeAsync(0);
        expect(host.querySelector("span[role='status']")).not.toBeNull();

        isRefused = false;
        const button = clickCopy(host);
        await vi.advanceTimersByTimeAsync(0);
        expect(host.querySelector("span[role='status']")).toBeNull();
        expect(button.textContent).toContain("Copied");
    });

    it("offers nothing while the value is empty", async () => {
        const writeText = vi.fn<(value: string) => Promise<void>>().mockResolvedValue(undefined);
        stubClipboard(writeText);
        const host = mount(() => <CopyButton value="" />);

        const button = requireElement<HTMLButtonElement>(host, "button");
        expect(button.disabled).toBe(true);
        button.click();
        await settle();
        expect(writeText).not.toHaveBeenCalled();
        expect(host.querySelector("span[role='status']")).toBeNull();
    });
});
