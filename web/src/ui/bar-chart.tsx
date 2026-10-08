import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, For } from "solid-js";
import { joinClass } from "/src/ui/attributes";

/** One bar: what it is called and what it measures. */
type BarChartPoint = {
    readonly label: string;
    readonly value: number;
};

/**
 * A bar chart of one series over time, drawn in the console's own materials:
 * accent bars on the card's ground, one 1px rule as the axis, and nothing the
 * data does not need — no gridlines, no legend, no second hue. Every bar is a
 * focusable point whose accessible name is its day and figure, and the readout
 * line under the axis states the window's own summary until a bar is read,
 * then that day and its figure — so a value is readable by pointer, by
 * keyboard, and by assistive technology, never by hovering alone.
 */
export function BarChart(props: {
    readonly points: ReadonlyArray<BarChartPoint>;
    /** What one bar counts, as each bar's accessible name spells it. */
    readonly describePoint: (point: BarChartPoint) => {
        readonly accessibleName: string;
        readonly content: JSX.Element;
    };
    /** The line stated while no bar is read: the window's own summary. */
    readonly summary: JSX.Element;
    readonly class?: string | undefined;
}): JSX.Element {
    // The bar being read — hovered or focused — or `null` for the summary.
    const [activeIndex, setActiveIndex] = createSignal<number | null>(null);

    const highest = createMemo(() =>
        props.points.reduce((most, point) => Math.max(most, point.value), 0),
    );

    /**
     * A bar's height: proportional to the window's busiest day, with a floor
     * so a quiet day still reads as a bar rather than vanishing.
     */
    const heightOf = (value: number): string => {
        if (value === 0 || highest() === 0) {
            return "2px";
        }

        return `${Math.max(4, Math.round((value / highest()) * 100))}%`;
    };

    /**
     * The readout line: the window's summary until a bar is read, then that
     * bar's day and figure. A day is one token — an ISO date broken across
     * lines is a broken date — so every day this line states stays unbreakable
     * while the sentence around it may still wrap.
     */
    const readout = (): JSX.Element => {
        const index = activeIndex();
        const point = index === null ? undefined : props.points[index];

        return point === undefined ? props.summary : props.describePoint(point).content;
    };

    return (
        <div class={joinClass("flex flex-col gap-2", props.class ?? "")}>
            <ul
                dir="ltr"
                class="flex h-28 items-end gap-1 border-b border-rule"
            >
                <For each={props.points}>
                    {(point, index) => (
                        <li class="flex h-full min-w-0 flex-1 items-end">
                            <span
                                tabindex="0"
                                role="img"
                                aria-label={props.describePoint(point).accessibleName}
                                class="flex h-full w-full items-end rounded-t-[3px] outline-offset-4"
                                onPointerEnter={() => setActiveIndex(index())}
                                onPointerLeave={() => setActiveIndex(null)}
                                onFocus={() => setActiveIndex(index())}
                                onBlur={() => setActiveIndex(null)}
                            >
                                <span
                                    aria-hidden="true"
                                    class={joinClass(
                                        "mx-auto block w-full max-w-12 rounded-t-[3px] transition-colors duration-150 ease-out",
                                        point.value === 0 ? "bg-rule-strong" : "bg-accent",
                                    )}
                                    style={{
                                        height: heightOf(point.value),
                                    }}
                                />
                            </span>
                        </li>
                    )}
                </For>
            </ul>
            <div
                dir="ltr"
                class="tnum flex justify-between text-[12.5px] text-ink-faint"
            >
                <bdi dir="ltr">{props.points[0]?.label ?? ""}</bdi>
                <bdi dir="ltr">{props.points.at(-1)?.label ?? ""}</bdi>
            </div>
            <p class="tnum text-[12.5px] text-ink-faint">{readout()}</p>
        </div>
    );
}
