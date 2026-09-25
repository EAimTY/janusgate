import type { JSX } from "@solidjs/web";

/**
 * The app's drawn marks: one authored set, one stroke weight, one 16-unit
 * grid. The app ships no icon library and no asset files, so every mark here
 * is geometry a reader can specify exactly. Status is never carried by color
 * alone, so the alert, info, and check marks are first-class citizens.
 */
export type GlyphName =
    | "chev-right"
    | "chev-left"
    | "plus"
    | "refresh"
    | "key"
    | "trash"
    | "check"
    | "close"
    | "search"
    | "out"
    | "alert"
    | "info"
    | "sun"
    | "moon"
    | "display"
    | "plug"
    | "copy"
    | "grid"
    | "stack"
    | "activity";

const GLYPH_PATHS: Record<GlyphName, string> = {
    "chev-right": "M6.5 4 L10.5 8 L6.5 12",
    "chev-left": "M9.5 4 L5.5 8 L9.5 12",
    plus: "M8 3.5 V12.5 M3.5 8 H12.5",
    refresh: "M12.6 8 A4.6 4.6 0 1 1 8 3.4 M8 3.4 L5.9 1.9 M8 3.4 L5.9 4.9",
    key: "M5.6 3.7 A1.9 1.9 0 1 1 5.59 3.71 Z M7 7 L13 13 M11 11 L12.4 9.6 M12.4 12.4 L13.8 11",
    trash: "M3.5 5 H12.5 M5.5 5 V12.5 H10.5 V5 M6.5 5 V3.5 H9.5 V5",
    check: "M4 8.5 L7 11.5 L12 4.5",
    close: "M5 5 L11 11 M11 5 L5 11",
    search: "M7.3 3.3 A4 4 0 1 1 7.29 3.31 Z M10.4 10.4 L13 13",
    out: "M6 4 H4 V12 H12 V10 M9 3.5 H12.5 V7 M12.5 3.5 L7.5 8.5",
    alert: "M8 2.5 L14.5 13.5 H1.5 Z M8 6.5 V10 M8 11.8 V12.2",
    info: "M8 1.8 A6.2 6.2 0 1 0 8.01 1.8 Z M8 7.4 V11 M8 4.9 V5.3",
    sun: "M8 2.2 A5.8 5.8 0 1 0 8.01 2.2 Z M8 5.4 A2.6 2.6 0 1 0 8.01 5.4 Z M8 0.8 V2 M8 14 V15.2 M0.8 8 H2 M14 8 H15.2 M2.9 2.9 L3.8 3.8 M12.2 12.2 L13.1 13.1 M13.1 2.9 L12.2 3.8 M3.8 12.2 L2.9 13.1",
    moon: "M13.5 9.5 A5.5 5.5 0 0 1 6.5 2.5 A5.5 5.5 0 1 0 13.5 9.5 Z",
    display: "M2.5 3 H13.5 V10.5 H2.5 Z M8 10.5 V13 M5 13 H11",
    plug: "M6 2 V5 M10 2 V5 M4.5 5 H11.5 V9 A3.5 3.5 0 0 1 4.5 9 Z M8 12.5 V14",
    copy: "M5.5 5.5 H12.5 V12.5 H5.5 Z M3.5 10.5 V3.5 H10.5",
    grid: "M2.5 2.5 H7 V7 H2.5 Z M9 2.5 H13.5 V7 H9 Z M2.5 9 H7 V13.5 H2.5 Z M9 9 H13.5 V13.5 H9 Z",
    stack: "M2.5 4.5 L8 2 L13.5 4.5 L8 7 Z M2.5 8 L8 10.5 L13.5 8 M2.5 11.5 L8 14 L13.5 11.5",
    activity: "M1.5 8 H5.5 L7.5 4.5 L9.5 11.5 L11.5 8 H14.5",
};

export function Glyph(props: { readonly name: GlyphName; readonly class?: string }): JSX.Element {
    return (
        <svg
            viewBox="0 0 16 16"
            width="14"
            height="14"
            fill="none"
            stroke="currentColor"
            stroke-width="1.25"
            stroke-linecap="square"
            stroke-linejoin="miter"
            aria-hidden="true"
            class={props.class}
        >
            <path d={GLYPH_PATHS[props.name]} />
        </svg>
    );
}

/**
 * The Janus mark: two gates mirrored about one axis, sharing the post between
 * them — one gateway, two facing directions. Drawn from exact coordinates so
 * the identity travels without any image file (PRODUCT.md brand commitment).
 */
export function JanusMark(props: { readonly size?: number; readonly class?: string }): JSX.Element {
    const size = props.size ?? 18;

    return (
        <svg
            viewBox="0 0 16 16"
            width={size}
            height={size}
            fill="none"
            stroke="currentColor"
            stroke-width="1.25"
            stroke-linecap="square"
            aria-hidden="true"
            class={props.class}
        >
            <path d="M1.5 14.5 V7.5 A3.25 3.25 0 0 1 8 7.5 V14.5" />
            <path d="M14.5 14.5 V7.5 A3.25 3.25 0 0 0 8 7.5 V14.5" />
            <path
                d="M1.5 14.5 H14.5"
                stroke="currentColor"
                opacity="0.55"
            />
        </svg>
    );
}
