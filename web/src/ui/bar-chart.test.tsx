import type { JSX } from "@solidjs/web";
import { render } from "@solidjs/web";
import { flush } from "solid-js";
import { afterEach, describe, expect, it } from "vitest";
import { BarChart } from "/src/ui/bar-chart";

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

afterEach(() => {
    disposeView?.();
    disposeView = null;
    document.body.replaceChildren();
});

const POINTS = [
    {
        label: "2026-09-24",
        value: 0,
    },
    {
        label: "2026-09-25",
        value: 50,
    },
    {
        label: "2026-09-26",
        value: 100,
    },
];

function chart(
    points: ReadonlyArray<{
        label: string;
        value: number;
    }>,
): JSX.Element {
    return (
        <BarChart
            points={points}
            describePoint={(point) => ({
                accessibleName: `${point.label}: ${point.value} requests`,
                content: <span>{`${point.label}: ${point.value} requests`}</span>,
            })}
            summary={<span>14 requests over the window</span>}
        />
    );
}

describe("BarChart", () => {
    it("draws one focusable, named point per bar, sized by the window's busiest day", () => {
        const host = mount(() => chart(POINTS));

        const bars = Array.from(host.querySelectorAll<HTMLElement>("span[role='img']"));
        expect(bars.length).toBe(3);
        expect(bars[0]?.getAttribute("aria-label")).toBe("2026-09-24: 0 requests");
        expect(bars[1]?.getAttribute("aria-label")).toBe("2026-09-25: 50 requests");
        expect(bars[2]?.getAttribute("aria-label")).toBe("2026-09-26: 100 requests");

        // Proportional to the busiest day, with a floor so a quiet day still
        // reads as a bar and a zero day never vanishes from the axis.
        expect(bars[0]?.querySelector("span[aria-hidden='true']")?.getAttribute("style")).toContain(
            "height: 2px",
        );
        expect(bars[1]?.querySelector("span[aria-hidden='true']")?.getAttribute("style")).toContain(
            "height: 50%",
        );
        expect(bars[2]?.querySelector("span[aria-hidden='true']")?.getAttribute("style")).toContain(
            "height: 100%",
        );
    });

    it("states the window's own summary until a bar is read, then that bar's day and figure", () => {
        const host = mount(() => chart(POINTS));

        const readout = requireElement(host, "p");
        expect(readout.textContent).toContain("14 requests over the window");

        const secondBar = Array.from(host.querySelectorAll<HTMLElement>("span[role='img']"))[1];
        if (secondBar === undefined) {
            throw new Error("Second bar missing.");
        }

        secondBar.focus();
        flush();
        expect(readout.textContent).toContain("2026-09-25: 50 requests");

        secondBar.blur();
        flush();
        expect(readout.textContent).toContain("14 requests over the window");
    });

    it("labels the axis with the window's first and last day", () => {
        const host = mount(() => chart(POINTS));

        const axis = requireElement(host, "div");
        expect(axis.textContent).toContain("2026-09-24");
        expect(axis.textContent).toContain("2026-09-26");
    });

    it("keeps an all-quiet window on its feet: every bar at the floor, no peak to state", () => {
        const host = mount(() =>
            chart([
                {
                    label: "2026-09-24",
                    value: 0,
                },
                {
                    label: "2026-09-25",
                    value: 0,
                },
            ]),
        );

        const bars = Array.from(host.querySelectorAll("span[role='img']"));
        for (const bar of bars) {
            expect(bar.querySelector("span[aria-hidden='true']")?.getAttribute("style")).toContain(
                "height: 2px",
            );
        }
    });
});
