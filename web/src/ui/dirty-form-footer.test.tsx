import type { JSX } from "@solidjs/web";
import { render } from "@solidjs/web";
import { flush } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DirtyFormFooter, HoldNotice } from "/src/ui/dirty-form-footer";

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

afterEach(() => {
    disposeView?.();
    disposeView = null;
    document.body.replaceChildren();
});

describe("DirtyFormFooter", () => {
    it("gates the save on the form's dirt and rules, and submits the form it closes", () => {
        const host = mount(() => (
            <DirtyFormFooter
                canSave={false}
                isSaving={false}
                isDirty={false}
                onRevert={vi.fn<() => void>()}
            />
        ));

        expect(buttonByLabel(host, "Save changes").disabled).toBe(true);
        expect(buttonByLabel(host, "Save changes").type).toBe("submit");
        expect(buttonByLabel(host, "Revert").disabled).toBe(true);
        // A clean form states nothing.
        expect(host.querySelector("[role='status']")).toBeNull();
    });

    it("offers the revert while edits stand and states them", () => {
        const onRevert = vi.fn<() => void>();
        const host = mount(() => (
            <DirtyFormFooter
                canSave={true}
                isSaving={false}
                isDirty={true}
                onRevert={onRevert}
            />
        ));

        expect(buttonByLabel(host, "Save changes").disabled).toBe(false);
        expect(requireElement(host, "[role='status']").textContent).toContain("Unsaved changes");

        buttonByLabel(host, "Revert").click();
        flush();
        expect(onRevert).toHaveBeenCalledTimes(1);
    });

    it("stays inert while the save is in flight and names it", () => {
        const onRevert = vi.fn<() => void>();
        const host = mount(() => (
            <DirtyFormFooter
                canSave={true}
                isSaving={true}
                isDirty={true}
                onRevert={onRevert}
            />
        ));

        const save = buttonByLabel(host, "Saving…");
        expect(save.disabled).toBe(true);
        expect(buttonByLabel(host, "Revert").disabled).toBe(true);
        // The dirt is being written; the standing reminder yields while it is.
        expect(host.querySelector("[role='status']")).toBeNull();

        save.click();
        flush();
        expect(onRevert).not.toHaveBeenCalled();
    });
});

describe("HoldNotice", () => {
    it("states the hold only while the row is held against a refresh", () => {
        const held = mount(() => (
            <HoldNotice
                isHeld={true}
                subject="key"
            />
        ));
        expect(held.textContent).toContain(
            "This key changed on the gateway while you have unsaved edits.",
        );
        expect(held.textContent).toContain(
            "Save writes your edits over the gateway's version; Revert picks the gateway's version up.",
        );

        const clean = mount(() => (
            <HoldNotice
                isHeld={false}
                subject="key"
            />
        ));
        expect(clean.textContent).toBe("");
    });
});
