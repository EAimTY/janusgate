import type { JSX } from "@solidjs/web";
import { AppLink } from "/src/ui/app-link";
import { Glyph } from "/src/ui/glyph";

/**
 * The one "the row is a link" card the list tables share: the row's
 * identifying lines, the badges naming its states, and the chevron pointing
 * at the row's own page. The tables own the content; this owns the card.
 */
export function RowLink(props: {
    readonly href: string;
    /** The row's heading line, styled by the table that knows its content. */
    readonly primary: JSX.Element;
    /** The row's quiet meta line beneath the heading. */
    readonly secondary: string;
    /** The badges naming the row's states, composed by the table. */
    readonly badges: JSX.Element;
}): JSX.Element {
    return (
        <AppLink
            href={props.href}
            class="group flex flex-wrap items-center gap-x-4 gap-y-2 rounded-xl border border-rule bg-surface px-4 py-3.5 transition-colors duration-150 ease-out hover:border-ink-faint"
        >
            <span class="min-w-0 flex-1 basis-56">
                {props.primary}
                <span class="tnum mt-0.5 block truncate text-[12.5px] text-ink-faint">
                    {props.secondary}
                </span>
            </span>
            <span class="flex items-center gap-2">{props.badges}</span>
            <Glyph
                name="chev-right"
                class="text-ink-faint transition-transform duration-150 ease-out group-hover:translate-x-0.5 group-hover:text-ink-dim"
            />
        </AppLink>
    );
}
