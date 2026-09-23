import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";
import { Notice } from "/src/ui/feedback";
import type { ScreenRead } from "./read";

/**
 * The standing notice a screen's read states (README §6.3): the last read
 * failure's diagnosis, or the last action's reported outcome, rendered where
 * every screen states it — above the screen's content, before the skeleton
 * window.
 */
export function ReadNotice(props: { readonly read: ScreenRead }): JSX.Element {
    return (
        <Show when={props.read.notice()}>
            {(current) => (
                <Notice
                    tone={current().tone}
                    title={current().title}
                    detail={current().detail}
                    fix={current().fix}
                />
            )}
        </Show>
    );
}
