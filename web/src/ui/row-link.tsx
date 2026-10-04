import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";
import { AppLink } from "/src/ui/app-link";
import { Glyph } from "/src/ui/glyph";

/**
 * The one "the row is a link" card the list tables share: the row's
 * identifying lines, the badges naming its states, and the chevron pointing
 * at the row's own page. The tables own the content; this owns the card.
 *
 * Without a trailing action the card is the anchor itself, and the table owns
 * the row element wrapping it. A trailing action — the copy button beside a
 * serving key's secret, say — must stay outside the anchor for the link's
 * keyboard and pointer semantics to survive, so it renders as the anchor's
 * sibling and this owns the row element pairing them.
 */
export function RowLink(props: {
    readonly href: string;
    /** The row's heading line, styled by the table that knows its content. */
    readonly primary: JSX.Element;
    /** The row's quiet meta line beneath the heading. */
    readonly secondary: JSX.Element;
    /**
     * The row's value line beneath the meta line — the datum the row exists
     * to show, one step stronger than the meta (a serving key's secret).
     * Renders only when given.
     */
    readonly tertiary?: string;
    /** The badges naming the row's states, composed by the table. */
    readonly badges: JSX.Element;
    /** The optional action rendered after the link, inside the row but never inside the anchor. */
    readonly trailing?: JSX.Element;
}): JSX.Element {
    // The anatomy both shapes share; the chevron's hover grammar must stay
    // defined here and nowhere else.
    const content = (
        <>
            <span class="min-w-0 flex-1 basis-56">
                {props.primary}
                <span class="tnum mt-0.5 block truncate text-[12.5px] text-ink-faint">
                    {props.secondary}
                </span>
                <Show when={props.tertiary !== undefined}>
                    <span class="tnum mt-1 block truncate text-[12.5px] text-ink-dim">
                        <bdi dir="ltr">{props.tertiary}</bdi>
                    </span>
                </Show>
            </span>
            <span class="flex min-w-0 max-w-full flex-wrap items-center gap-2">{props.badges}</span>
            <Glyph
                name="chev-right"
                class="text-ink-faint transition-transform duration-150 ease-out group-hover:translate-x-0.5 rtl:group-hover:-translate-x-0.5 group-hover:text-ink-dim"
            />
        </>
    );

    if (props.trailing === undefined) {
        return (
            <AppLink
                href={props.href}
                class="group flex flex-wrap items-center gap-x-4 gap-y-2 rounded-xl border border-rule bg-surface px-4 py-3.5 transition-colors duration-150 ease-out hover:border-ink-faint"
            >
                {content}
            </AppLink>
        );
    }

    return (
        <li class="flex flex-wrap items-center gap-x-4 gap-y-2 rounded-xl border border-rule bg-surface px-4 py-3.5 transition-colors duration-150 ease-out hover:border-ink-faint">
            <AppLink
                href={props.href}
                class="group flex min-w-0 flex-1 basis-56 flex-wrap items-center gap-x-4 gap-y-2"
            >
                {content}
            </AppLink>
            {props.trailing}
        </li>
    );
}
