import type { JSX } from "@solidjs/web";
import { render } from "@solidjs/web";
import { afterEach, describe, expect, it } from "vitest";
import { RowLink } from "/src/ui/row-link";

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

function rowLink(
    options: { readonly tertiary?: string; readonly trailing?: JSX.Element } = {},
): JSX.Element {
    return (
        <RowLink
            href="/api-keys/7"
            primary={<span>staging</span>}
            secondary="Created Jan 1, 2026, 12:00 PM"
            badges={<span>On</span>}
            {...(options.tertiary === undefined
                ? {}
                : {
                      tertiary: options.tertiary,
                  })}
            {...(options.trailing === undefined
                ? {}
                : {
                      trailing: options.trailing,
                  })}
        />
    );
}

describe("RowLink", () => {
    it("is the row's anchor: heading, meta, and badges inside one link to the row's page", () => {
        const host = mount(() => rowLink({}));

        const anchor = host.querySelector("a");
        expect(anchor?.getAttribute("href")).toBe("/api-keys/7");
        expect(anchor?.textContent).toContain("staging");
        expect(anchor?.textContent).toContain("Created Jan 1, 2026, 12:00 PM");
        expect(anchor?.textContent).toContain("On");
    });

    it("carries the value line left-to-right while it is given", () => {
        const host = mount(() =>
            rowLink({
                tertiary: "sk-staging-0001",
            }),
        );

        const value = host.querySelector("bdi[dir='ltr']");
        expect(value?.textContent).toBe("sk-staging-0001");
    });

    it("pairs a trailing action beside the anchor, never inside it", () => {
        const host = mount(() =>
            rowLink({
                trailing: <button type="button">Copy</button>,
            }),
        );

        // The row element owns the pair, so the action's clicks and the
        // link's keyboard semantics stay independent.
        expect(host.querySelector("li")).not.toBeNull();
        const anchor = host.querySelector("a");
        const copy = Array.from(host.querySelectorAll("button")).find(
            (button) => button.textContent === "Copy",
        );
        expect(anchor?.textContent).not.toContain("Copy");
        expect(anchor?.contains(copy ?? null)).toBe(false);
        expect(copy?.textContent).toBe("Copy");
    });
});
