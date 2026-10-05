import type { JSX } from "@solidjs/web";
import { render } from "@solidjs/web";
import { describe, expect, it } from "vitest";
import { createScreenRead } from "../screen-state/read";
import { createDisposals, settle } from "../test-support/management-api";
import { SkeletonRows } from "./skeleton-rows";

/** A minimal list screen: the rows stand in for whatever its read fetches. */
function RowsScreen(props: {
    readonly load: (signal: AbortSignal) => Promise<void>;
    readonly rowCount: number;
    readonly rowHeight: string;
}): JSX.Element {
    const read = createScreenRead({
        load: props.load,
    });

    return (
        <SkeletonRows
            read={read}
            rowCount={props.rowCount}
            rowHeight={props.rowHeight}
        />
    );
}

/** The placeholder rows, by their placeholder construction. */
function skeletonRows(): Array<Element> {
    return [
        ...document.body.querySelectorAll("div[aria-hidden='true'].wash"),
    ];
}

describe("SkeletonRows", () => {
    const disposes = createDisposals();

    it("stands in rowCount rows at rowHeight while the first read is in flight", async () => {
        disposes.push(
            render(
                () => (
                    <RowsScreen
                        load={() => new Promise<void>(() => {})}
                        rowCount={2}
                        rowHeight="h-24"
                    />
                ),
                document.body,
            ),
        );
        await settle();

        const rows = skeletonRows();
        expect(rows).toHaveLength(2);
        for (const row of rows) {
            expect(row.className).toContain("h-24");
            expect(row.className).toContain("w-full");
            expect(row.className).toContain("rounded-xl");
        }
    });

    it("closes the rows once the first read lands", async () => {
        disposes.push(
            render(
                () => (
                    <RowsScreen
                        load={async () => {}}
                        rowCount={3}
                        rowHeight="h-16"
                    />
                ),
                document.body,
            ),
        );
        await settle();

        expect(skeletonRows()).toHaveLength(0);
    });
});
