import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";
import type { ScreenRead } from "../screen-state/read";
import { Skeleton } from "./feedback";

/**
 * The list screens' shared first-read skeleton (`web/docs/architecture.md`): placeholder
 * rows standing in for the listing while the first read is in flight, closed
 * the moment it lands — the list counterpart of the frame's skeleton window.
 * What varies is how many rows stand in and at what height.
 */
export function SkeletonRows(props: {
    /** The screen's read, whose first-read window opens and closes the rows. */
    readonly read: ScreenRead;
    /** How many placeholder rows stand in for the listing. */
    readonly rowCount: number;
    /** The height class one placeholder row stands at — the height of the rows it stands in for. */
    readonly rowHeight: string;
}): JSX.Element {
    return (
        <Show when={props.read.isFirstRead()}>
            <div class="flex flex-col gap-2">
                <For
                    each={Array.from({
                        length: props.rowCount,
                    })}
                >
                    {() => <Skeleton class={`${props.rowHeight} w-full rounded-xl`} />}
                </For>
            </div>
        </Show>
    );
}
