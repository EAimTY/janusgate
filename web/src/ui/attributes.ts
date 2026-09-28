import type { JSX } from "@solidjs/web";

/**
 * Solid's `class` takes one string and form elements take no array, so every
 * conditional class in the panel is composed through this join instead.
 */
export function joinClass(
    ...parts: ReadonlyArray<JSX.ClassValue | undefined | null | false>
): string {
    return parts
        .filter((part): part is string => typeof part === "string" && part !== "")
        .join(" ");
}

/**
 * The one class of an in-prose link — an anchor inside a notice's detail or
 * a paragraph's copy: medium-weight accent text that shifts to the stronger
 * accent on hover. Every such link composes this constant, so the treatment
 * cannot drift between screens.
 */
export const proseLinkClass = "font-medium text-accent hover:text-accent-strong";

/**
 * ARIA states are enumerated strings in the DOM typings, not booleans; this
 * keeps the panel's switches honest about it.
 */
export function ariaBoolean(value: boolean): "true" | "false" {
    return value ? "true" : "false";
}
