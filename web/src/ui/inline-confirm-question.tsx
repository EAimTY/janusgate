import type { JSX } from "@solidjs/web";
import type { InlineConfirmFocus } from "./inline-confirm";

/**
 * The armed question every inline confirm states while armed. The element is
 * the load-bearing half of the shared inline-confirm focus contract
 * (`./inline-confirm`): it carries `tabindex="-1"` and the `focusArmed`
 * ref, so arming can move focus onto the question and a screen reader reads
 * the question before the confirmation button beside it. The armed branches
 * render this fragment instead of re-spelling the element, so the contract's
 * attributes live in one place beside the contract.
 */
export function InlineConfirmQuestion(props: {
    readonly question: JSX.Element;
    readonly focusArmed: InlineConfirmFocus["focusArmed"];
}): JSX.Element {
    return (
        <p
            class="w-full text-[13.5px] text-ink-dim"
            tabindex={-1}
            ref={props.focusArmed}
        >
            {props.question}
        </p>
    );
}
