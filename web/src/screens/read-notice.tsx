import type { JSX } from "@solidjs/web";
import { type Accessor, Show } from "solid-js";
import { translation } from "/src/i18n";
import { Button } from "/src/ui/controls";
import { Notice } from "/src/ui/feedback";
import type { ScreenNotice, ScreenRead } from "./read";

/**
 * One stated notice rendered as its `Notice`: the single `ScreenNotice` →
 * `Notice` mapping every standing notice walks — the read-level notice above
 * the screen's content (`ReadNotice`) and each locally-owned action outcome
 * alike — so a change to how a stated notice renders is made once. With
 * neither `class` nor `children`, the notice renders bare, exactly where the
 * screen states it. With either, it stands in a `div` rendered only while a
 * notice stands — `class` carrying the placement wrapper's own styling (a
 * card's `mt-4`, the action wrapper's flex column) and `children` stated
 * below the notice, where its placement offers an action beside it — while
 * `noticeClass` classes the `Notice` element itself.
 */
export function StatedNotice(props: {
    readonly notice: Accessor<ScreenNotice | null>;
    readonly class?: string | undefined;
    readonly noticeClass?: string | undefined;
    readonly children?: JSX.Element;
}): JSX.Element {
    return (
        <Show when={props.notice()}>
            {(current) => {
                const notice = (
                    <Notice
                        tone={current().tone}
                        title={current().title}
                        detail={current().detail}
                        rawDetail={current().rawDetail}
                        fix={current().fix}
                        class={props.noticeClass}
                    />
                );

                if (props.class === undefined && props.children === undefined) {
                    return notice;
                }

                return (
                    <div class={props.class}>
                        {notice}
                        {props.children}
                    </div>
                );
            }}
        </Show>
    );
}

/**
 * The standing notice a screen's read states (README §6.3): the last read
 * failure's diagnosis, or the last action's reported outcome, rendered where
 * every screen states it — above the screen's content, before the skeleton
 * window.
 *
 * Where the screen wires its reload as `retry`, a failed first read carries
 * the same *Read again* action the detail and edit frame renders beside its
 * standing failure (README §6.3): the same wording, rerunning the screen's
 * own read in place — the recovery needs no trip to the shell's *Refresh*.
 * The action gates on the first read exactly as the frame's does, so it
 * stands only where the failure leaves nothing else on the page; a notice
 * beside last-good content (a failed re-read) or an action's reported
 * outcome states itself without offering a read rerun.
 */
export function ReadNotice(props: {
    readonly read: ScreenRead;
    /**
     * The screen's own reload, offered as the failed first read's *Read
     * again* action. The harness's reload is safe to call from the failure
     * state: it aborts any cycle still in flight and re-runs the read.
     */
    readonly retry?: (() => void) | undefined;
}): JSX.Element {
    return (
        <StatedNotice
            notice={props.read.notice}
            class="flex flex-col items-start gap-3"
            noticeClass="w-full"
        >
            {/* The frame's gate over its own read-again branch: the
                action belongs to a failure that closed the skeleton
                and left nothing else standing, never to a notice an
                action stated or one standing beside last-good
                content. */}
            <Show when={props.retry !== undefined && !props.read.hasRead()}>
                <Button
                    glyph="refresh"
                    tone="ghost"
                    isBusy={props.read.isReading()}
                    onClick={() => props.retry?.()}
                >
                    {translation()(($) => $.screens.readNotice.readAgain)}
                </Button>
            </Show>
        </StatedNotice>
    );
}
