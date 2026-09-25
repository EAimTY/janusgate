import type { JSX } from "@solidjs/web";
import type { Accessor } from "solid-js";
import { Show } from "solid-js";
import { Skeleton } from "/src/ui/feedback";
import type { ScreenRead } from "./read";
import { ReadNotice } from "./read-notice";

/**
 * The row screens' shared frame (README §6.3): the standing read notice, the
 * first read's skeleton window, the read-but-gone not-found state, and the
 * keyed row content — the three-branch presentation every detail and edit
 * screen states around its row, in that order, with the branches mutually
 * exclusive by their own conditions. The header and not-found slots stay
 * render slots so each screen keeps its own titles, routes, and ways back;
 * the frame owns the branches, the skeleton window (the read harness's
 * first-read window, identical on every row screen), and nothing else.
 */
export function ScreenFrame<TRow>(props: {
    readonly read: ScreenRead;
    /** The row the screen addresses, null while the row is unread or gone. */
    readonly row: Accessor<TRow | null>;
    /** The header above the skeleton, shown while the first read is in flight. */
    readonly firstReadHeader: JSX.Element;
    /** Everything shown when the read landed but no row answers the address. */
    readonly notFound: JSX.Element;
    /** The screen's content on the row, keyed to the row's identity. */
    readonly children: (row: TRow) => JSX.Element;
}): JSX.Element {
    return (
        <div class="rise-in flex flex-col gap-5">
            <ReadNotice read={props.read} />

            <Show when={props.read.isFirstRead()}>
                {props.firstReadHeader}
                <Skeleton class="h-64 w-full rounded-2xl" />
            </Show>

            <Show when={props.read.hasRead() && props.row() === null}>{props.notFound}</Show>

            <Show
                when={props.row()}
                keyed
            >
                {(row) => props.children(row)}
            </Show>
        </div>
    );
}
