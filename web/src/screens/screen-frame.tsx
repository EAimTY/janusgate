import type { JSX } from "@solidjs/web";
import type { Accessor } from "solid-js";
import { Show } from "solid-js";
import { Button } from "/src/ui/controls";
import { Skeleton } from "/src/ui/feedback";
import type { ScreenRead } from "./read";
import { ReadNotice } from "./read-notice";

/**
 * The row screens' shared frame (README §6.3): the standing read notice, the
 * first read's skeleton window, the failed first read's own presentation, the
 * read-but-gone not-found state, and the keyed row content — the presentation
 * every detail and edit screen states around its row, with the branches
 * mutually exclusive by their own conditions. The header and not-found slots
 * stay render slots so each screen keeps its own titles, routes, and ways
 * back; the frame owns the branches, the skeleton window (the read harness's
 * first-read window, identical on every row screen), and nothing else.
 */
export function ScreenFrame<TRow>(props: {
    readonly read: ScreenRead;
    /** The row the screen addresses, null while the row is unread or gone. */
    readonly row: Accessor<TRow | null>;
    /**
     * The screen's own header: above the skeleton while the first read is in
     * flight, and kept up beside the standing failure when the first read
     * fails — the page identity and its way back in both states.
     */
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

            {/*
                A first read that failed leaves no row and no skeleton (the
                standing failure holds the skeleton window closed), which
                alone would strand the user on an unnamed page. The screen's
                own header — the identity and the way back — stays up beside
                the failure, and a read-again action reruns the read in place.
                The failure stands through the re-read it starts, so this
                branch holds open (and the skeleton stays closed) until a read
                finally lands. */}
            <Show when={!props.read.hasRead() && props.read.notice() !== null}>
                {props.firstReadHeader}
                <div>
                    <Button
                        glyph="refresh"
                        isBusy={props.read.isReading()}
                        onClick={() => props.read.reload()}
                    >
                        Read again
                    </Button>
                </div>
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
