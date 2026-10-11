import type { JSX } from "@solidjs/web";
import { render } from "@solidjs/web";
import { afterEach, describe, expect, it } from "vitest";
import { RowNotFound } from "/src/ui/row-not-found";

let disposeView: (() => void) | null = null;

function mount(view: () => JSX.Element): HTMLElement {
    const host = document.createElement("div");
    document.body.appendChild(host);
    disposeView = render(view, host);
    return host;
}

afterEach(() => {
    disposeView?.();
    disposeView = null;
    document.body.replaceChildren();
});

describe("RowNotFound", () => {
    it("keeps the screen's own header over the row kind's not-found state, with the way back", () => {
        const host = mount(() => (
            <RowNotFound
                kind="api_key"
                headerTitle="Edit API key"
            />
        ));

        // The screen's own header — the identity and the way back — stays up.
        expect(host.querySelector("h1")?.textContent).toBe("Edit API key");
        const back = Array.from(host.querySelectorAll("a")).find(
            (link) => link.getAttribute("href") === "/api-keys",
        );
        expect(back?.textContent).toContain("API keys");
        expect(host.textContent).toContain("API key not found");
        expect(host.textContent).toContain(
            "The gateway holds no API key here. It was deleted, or the address was typed wrong. Nothing here can bring it back.",
        );
        // The primary action is the listing, worded as the way back.
        expect(host.textContent).toContain("Back to API keys");
    });

    it("offers the creation action only where a replacement is a way forward", () => {
        const withCreate = mount(() => (
            <RowNotFound
                kind="api_key"
                headerTitle="API key"
                createAction={{
                    href: "/api-keys/new",
                    label: "Create API key",
                }}
            />
        ));
        const create = Array.from(withCreate.querySelectorAll("a")).find(
            (link) => link.getAttribute("href") === "/api-keys/new",
        );
        expect(create?.textContent).toBe("Create API key");

        const withoutCreate = mount(() => (
            <RowNotFound
                kind="provider"
                headerTitle="Edit provider"
            />
        ));
        expect(withoutCreate.querySelectorAll("a[href='/providers/new']").length).toBe(0);
        expect(withoutCreate.textContent).toContain("Provider not found");
    });
});
