import { afterEach, describe, expect, it, vi } from "vitest";
import { copyText } from "./clipboard";

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

afterEach(() => {
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

        await expect(copyText("sk-live")).resolves.toBe(true);

        expect(writeText).toHaveBeenCalledWith("sk-live");
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
