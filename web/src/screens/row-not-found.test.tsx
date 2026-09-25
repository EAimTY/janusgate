import { render } from "@solidjs/web";
import { afterEach, describe, expect, it } from "vitest";
import { RowNotFound } from "./row-not-found";

describe("RowNotFound", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    it("states the row kind's copy under the screen's own header, with the way back to the listing", () => {
        disposes.push(
            render(
                () => (
                    <RowNotFound
                        kind="provider"
                        headerTitle="Edit provider"
                    />
                ),
                document.body,
            ),
        );

        // The header keeps the page's identity, the copy is the kind's, and
        // the way back — in the header and as the primary action — leads to
        // the listing. No creation door without a route for one.
        expect(document.body.querySelector("h1")?.textContent).toBe("Edit provider");
        expect(document.body.textContent).toContain("Provider not found");
        expect(document.body.textContent).toContain(
            "The gateway holds no provider here. It was deleted — on this page, in another session, or by hand. Nothing here can bring it back.",
        );
        // The way back leads to the listing twice — the header's back link
        // and the primary action — and the primary action carries the
        // composed label.
        const listingLinks = [
            ...document.body.querySelectorAll("a[href='/providers']"),
        ];
        expect(listingLinks.map((link) => link.textContent?.trim())).toContain("Back to providers");
        expect(document.body.textContent).not.toContain("Create provider");
    });

    it("adds the creation action only where the screen passes one", () => {
        disposes.push(
            render(
                () => (
                    <RowNotFound
                        kind="api_key"
                        headerTitle="API key"
                        createAction={{
                            href: "/api-keys/new",
                            label: "Create API key",
                        }}
                    />
                ),
                document.body,
            ),
        );

        expect(document.body.textContent).toContain("API key not found");
        expect(document.body.querySelector("a[href='/api-keys/new']")?.textContent).toContain(
            "Create API key",
        );
    });
});
