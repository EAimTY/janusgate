import type { JSX } from "@solidjs/web";
import { render } from "@solidjs/web";
import { flush } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ConfirmDeleteCard } from "/src/ui/confirm-delete-card";

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

function buttonByLabel(host: ParentNode, label: string): HTMLButtonElement {
    const button = Array.from(host.querySelectorAll("button")).find((candidate) =>
        candidate.textContent?.includes(label),
    );
    if (button === undefined) {
        throw new Error(`No button labeled "${label}".`);
    }

    return button;
}

/** Clicks and lands Solid's staged writes, so the DOM answers before the test looks. */
function click(element: HTMLElement): void {
    element.click();
    flush();
}

/**
 * Solid stages imperative writes and the inline-confirm focus work defers one
 * macrotask past the branch swap it follows; two timer rounds settle both.
 */
async function flushMacrotasks(): Promise<void> {
    await new Promise((resolve) => {
        setTimeout(resolve, 0);
    });
    await new Promise((resolve) => {
        setTimeout(resolve, 0);
    });
}

afterEach(() => {
    disposeView?.();
    disposeView = null;
    document.body.replaceChildren();
});

describe("ConfirmDeleteCard", () => {
    it("arms where it states its blast radius and disarms back to the trigger", async () => {
        const host = mount(() => (
            <ConfirmDeleteCard
                title="Delete this key"
                description="Deleting revokes the key at once."
                question="Delete staging? Clients holding this secret lose access."
                onConfirm={vi.fn<() => Promise<boolean>>().mockResolvedValue(true)}
            />
        ));

        // The armed question takes the focus the arming trigger held.
        const trigger = buttonByLabel(host, "Delete…");
        trigger.focus();
        click(trigger);
        await flushMacrotasks();

        const question = requireElement<HTMLParagraphElement>(host, "p[tabindex='-1']");
        expect(question.textContent).toContain(
            "Delete staging? Clients holding this secret lose access.",
        );
        expect(document.activeElement).toBe(question);

        click(buttonByLabel(host, "Keep it"));
        await flushMacrotasks();

        expect(host.querySelector("p[tabindex='-1']")).toBeNull();
        // The swap remounts the trigger, and focus returns to it.
        expect(document.activeElement).toBe(buttonByLabel(host, "Delete…"));
    });

    it("disarms when the answered deletion reports the row survived", async () => {
        const onConfirm = vi.fn<() => Promise<boolean>>().mockResolvedValue(false);
        const host = mount(() => (
            <ConfirmDeleteCard
                title="Delete this key"
                description="Deleting revokes the key at once."
                question="Delete staging?"
                onConfirm={onConfirm}
            />
        ));

        click(buttonByLabel(host, "Delete…"));
        await flushMacrotasks();
        click(buttonByLabel(host, "Delete for good"));
        await flushMacrotasks();

        expect(onConfirm).toHaveBeenCalledTimes(1);
        expect(host.querySelector("p[tabindex='-1']")).toBeNull();
        expect(buttonByLabel(host, "Delete…").disabled).toBe(false);
    });

    it("states the busy deletion while the confirm is in flight", async () => {
        let resolveDeletion: (isDeleted: boolean) => void = () => {};
        const host = mount(() => (
            <ConfirmDeleteCard
                title="Delete this key"
                description="Deleting revokes the key at once."
                question="Delete staging?"
                onConfirm={() =>
                    new Promise<boolean>((resolve) => {
                        resolveDeletion = resolve;
                    })
                }
            />
        ));

        click(buttonByLabel(host, "Delete…"));
        await flushMacrotasks();
        click(buttonByLabel(host, "Delete for good"));
        await flushMacrotasks();

        const confirm = buttonByLabel(host, "Deleting…");
        expect(confirm.disabled).toBe(true);
        expect(buttonByLabel(host, "Keep it").disabled).toBe(true);

        // While the deletion is in flight a further click runs nothing.
        confirm.click();
        expect(
            requireElement<HTMLParagraphElement>(host, "p[tabindex='-1']").textContent,
        ).toContain("Delete staging?");

        resolveDeletion(true);
        await flushMacrotasks();
        // The settled deletion disengages the busy state; the armed question
        // stays up (a confirmed deletion leaves by navigating away).
        expect(buttonByLabel(host, "Delete for good").disabled).toBe(false);
    });
});
