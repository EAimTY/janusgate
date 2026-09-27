import { render } from "@solidjs/web";
import { afterEach, describe, expect, it } from "vitest";
import { RowLink } from "/src/ui/row-link";
import { buttonWithText, linkTo, settle } from "../screens/screen-test-support";

describe("RowLink trailing action", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    it("renders the trailing action inside the row but outside the link", async () => {
        disposes.push(
            render(
                () => (
                    <ul>
                        <RowLink
                            href="/keys/1"
                            primary={<span>alpha</span>}
                            secondary="Created just now"
                            tertiary="sk-alpha"
                            badges={<span>On</span>}
                            trailing={<button type="button">Copy</button>}
                        />
                    </ul>
                ),
                document.body,
            ),
        );
        await settle();

        const link = linkTo("/keys/1");
        const action = buttonWithText("Copy");
        const row = link.closest("li");

        // The action shares the row with the link but is no descendant of it:
        // a button inside an anchor would break both keyboard paths.
        expect(row).not.toBeNull();
        expect(row?.contains(action)).toBe(true);
        expect(link.contains(action)).toBe(false);
    });
});
