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
 * ARIA states are enumerated strings in the DOM typings, not booleans; this
 * keeps the panel's switches honest about it.
 */
export function ariaBoolean(value: boolean): "true" | "false" {
    return value ? "true" : "false";
}
