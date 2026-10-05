import { render } from "@solidjs/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import { buttonWithText, createDisposals } from "../test-support/management-api";
import { CopyButton, copyText } from "./clipboard";

/**
 * The clipboard is exercised against stubs: the helper's contract is the
 * fallback ladder — the async Clipboard API first, the legacy `execCommand`
 * textarea second — and the honesty of its boolean. Real browser permission
 * semantics are covered by the browser-level verification against a running
 * instance.
 */
function stubClipboard(
    clipboard:
        | {
              writeText: (value: string) => Promise<void>;
          }
        | undefined,
): void {
    Object.defineProperty(navigator, "clipboard", {
        value: clipboard,
        configurable: true,
    });
}

function stubExecCommand(isCopied: boolean) {
    const execCommand = vi.fn(() => isCopied);
    Object.defineProperty(document, "execCommand", {
        value: execCommand,
        configurable: true,
    });

    return execCommand;
}

const disposes = createDisposals();

afterEach(() => {
    vi.useRealTimers();
    delete (
        navigator as {
            clipboard?: unknown;
        }
    ).clipboard;
    delete (
        document as {
            execCommand?: unknown;
        }
    ).execCommand;
});

describe("copyText", () => {
    it("writes through the async Clipboard API when the browser offers it", async () => {
        const writeText = vi.fn(() => Promise.resolve());
        stubClipboard({
            writeText,
        });
        const execCommand = stubExecCommand(true);

        const mixedValue = "sk-live/العربية 中文 [[name]] <script>";
        await expect(copyText(mixedValue)).resolves.toBe(true);

        expect(writeText).toHaveBeenCalledWith(mixedValue);
        expect(execCommand).not.toHaveBeenCalled();
    });

    it("copies through a transient textarea when no async Clipboard API exists", async () => {
        stubClipboard(undefined);
        const state: {
            areaAtCopyTime: HTMLTextAreaElement | null;
        } = {
            areaAtCopyTime: null,
        };
        Object.defineProperty(document, "execCommand", {
            value: vi.fn(() => {
                state.areaAtCopyTime = document.querySelector("textarea");
                return true;
            }),
            configurable: true,
        });

        await expect(copyText("sk-live")).resolves.toBe(true);

        expect(state.areaAtCopyTime?.value).toBe("sk-live");
        // Safari's `select()` implicitly focuses the textarea; it must be
        // inert and pinned fixed off-screen so that focus cannot scroll the
        // document to it.
        expect(state.areaAtCopyTime?.hasAttribute("readonly")).toBe(true);
        expect(state.areaAtCopyTime?.style.position).toBe("fixed");
        expect(state.areaAtCopyTime?.style.top).toBe("-9999px");
        expect(state.areaAtCopyTime?.style.left).toBe("-9999px");
        expect(document.querySelectorAll("textarea")).toHaveLength(0);
    });

    it("falls back to the textarea path when the browser refuses the async write", async () => {
        stubClipboard({
            writeText: () => Promise.reject(new DOMException("denied", "NotAllowedError")),
        });
        const execCommand = stubExecCommand(true);

        await expect(copyText("sk-live")).resolves.toBe(true);

        expect(execCommand).toHaveBeenCalledWith("copy");
    });

    it("reports the failure when both paths refuse the value", async () => {
        stubClipboard({
            writeText: () => Promise.reject(new DOMException("denied", "NotAllowedError")),
        });
        stubExecCommand(false);

        await expect(copyText("sk-live")).resolves.toBe(false);
    });
});

describe("CopyButton", () => {
    /** Mounts one CopyButton over a fixed value, naming `subject` when given. */
    function mountCopyButton(subject?: string): void {
        disposes.push(
            render(
                () => (
                    <CopyButton
                        value="sk-live"
                        subject={subject}
                    />
                ),
                document.body,
            ),
        );
    }

    it("confirms a successful copy on the label and reverts shortly after", async () => {
        vi.useFakeTimers();
        stubClipboard({
            writeText: () => Promise.resolve(),
        });
        mountCopyButton();

        buttonWithText("Copy").click();
        await vi.advanceTimersByTimeAsync(0);

        expect(document.body.textContent).toContain("Copied");
        expect(document.body.textContent).not.toContain("refused the clipboard");

        await vi.advanceTimersByTimeAsync(1500);

        expect(document.body.textContent).not.toContain("Copied");
        expect(buttonWithText("Copy").textContent).toContain("Copy");
    });

    it("names the subject in a refusal that stands until a later copy succeeds", async () => {
        vi.useFakeTimers();
        let isDenied = true;
        stubClipboard({
            writeText: () =>
                isDenied
                    ? Promise.reject(new DOMException("denied", "NotAllowedError"))
                    : Promise.resolve(),
        });
        stubExecCommand(false);
        mountCopyButton("the URL above");

        buttonWithText("Copy").click();
        await vi.advanceTimersByTimeAsync(0);

        expect(document.body.textContent).toContain(
            "This browser refused the clipboard — select the URL above and copy it by hand.",
        );

        // The refusal is not on the copied label's clock; it stands until a
        // later copy says something else.
        await vi.advanceTimersByTimeAsync(1500);
        expect(document.body.textContent).toContain("refused the clipboard");

        isDenied = false;
        buttonWithText("Copy").click();
        await vi.advanceTimersByTimeAsync(0);

        expect(document.body.textContent).not.toContain("refused the clipboard");
        expect(document.body.textContent).toContain("Copied");

        await vi.advanceTimersByTimeAsync(1500);
        expect(document.body.textContent).not.toContain("Copied");
    });
});
