import type { JSX } from "@solidjs/web";
import { joinClass } from "./attributes";

/**
 * The one masked-secret reveal: a checkbox under the masked field that lifts
 * the mask while the operator confirms a paste. The field owns the visibility
 * signal — every mount opens masked — and this control only reports the
 * checkbox's checked state to it, so `isVisible` stays the field's truth and
 * the caption stays the caller's context (what the mask is hiding). The
 * optional spacing class is the surrounding layout's business: forms that
 * stack their fields on a flex gap need none, while a field standing in a
 * plain block adds its own.
 */
export function SecretRevealToggle(props: {
    readonly caption: string;
    readonly isVisible: boolean;
    readonly onToggle: (isVisible: boolean) => void;
    readonly class?: string;
}): JSX.Element {
    return (
        <label
            class={joinClass(
                "flex w-fit cursor-pointer items-center gap-2 text-[13.5px] text-ink-dim",
                props.class,
            )}
        >
            <input
                dir="ltr"
                type="checkbox"
                class="size-4 accent-(--color-accent)"
                checked={props.isVisible}
                onChange={(event) => props.onToggle(event.currentTarget.checked)}
            />
            {props.caption}
        </label>
    );
}
