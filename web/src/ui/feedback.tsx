import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";
import { AppLink } from "/src/ui/app-link";
import { joinClass } from "/src/ui/attributes";
import { Glyph, type GlyphName } from "/src/ui/glyph";

export type NoticeTone = "ok" | "warn" | "bad" | "info";

const NOTICE_CLASS: Record<NoticeTone, string> = {
    ok: "border-ok/30 bg-ok-soft text-ink",
    warn: "border-warn/30 bg-warn-soft text-ink",
    bad: "border-bad/30 bg-bad-soft text-ink",
    info: "border-rule-strong bg-muted text-ink",
};

const NOTICE_GLYPH: Record<NoticeTone, GlyphName> = {
    ok: "check",
    warn: "alert",
    bad: "alert",
    info: "info",
};

/**
 * What the last action did, stated where the action was taken. The title is
 * the app's own statement, the detail is the gateway's message verbatim (or
 * the notice's own prose, linked where a recovery path is a route), and the
 * fix is named only where the app can be certain of one.
 */
export function Notice(props: {
    readonly tone: NoticeTone;
    readonly title: JSX.Element;
    readonly detail?: string | JSX.Element | null | undefined;
    readonly fix?: JSX.Element;
    readonly rawDetail?: string | undefined;
    readonly class?: string | undefined;
}): JSX.Element {
    return (
        <div
            class={joinClass(
                "flex items-start gap-2.5 rounded-xl border px-3.5 py-3",
                NOTICE_CLASS[props.tone],
                props.class ?? "",
            )}
            role={props.tone === "bad" ? "alert" : "status"}
        >
            <Glyph
                name={NOTICE_GLYPH[props.tone]}
                class="mt-0.5 shrink-0"
            />
            <div class="min-w-0 flex-1">
                <p class="text-[13.5px] font-medium">{props.title}</p>
                <Show when={props.detail}>
                    {(detail) => <p class="mt-1 break-words text-ink-dim">{detail()}</p>}
                </Show>
                <Show when={props.rawDetail}>
                    {(detail) => (
                        <pre class="mt-1 whitespace-pre-wrap break-words text-[12.5px] text-ink-dim">
                            <bdi dir="auto">{detail()}</bdi>
                        </pre>
                    )}
                </Show>
                <Show when={props.fix}>
                    {(fix) => <p class="mt-1 max-w-[70ch] text-[13px] text-ink-dim">{fix()}</p>}
                </Show>
            </div>
        </div>
    );
}

export type BadgeTone = "ok" | "warn" | "bad" | "muted" | "info";

const BADGE_CLASS: Record<BadgeTone, string> = {
    ok: "bg-ok-soft text-ok",
    warn: "bg-warn-soft text-warn",
    bad: "bg-bad-soft text-bad",
    muted: "bg-muted text-ink-dim",
    info: "bg-accent-soft text-accent dark:text-accent-strong",
};

const BADGE_GLYPH: Record<BadgeTone, GlyphName> = {
    ok: "check",
    warn: "alert",
    bad: "close",
    muted: "info",
    info: "info",
};

/**
 * The tone-and-words pair a badge-deriving helper answers, named once beside
 * `Badge` so every helper and consumer types against one record and a field
 * added to a derivation propagates instead of splitting across private twins.
 */
export type BadgeMark = {
    readonly tone: BadgeTone;
    readonly label: string;
};

/**
 * A status that reads as icon plus words, never color alone. The optional
 * title rides the native tooltip, for a state whose one word needs its
 * sentence carried on hand where no room to state it inline exists.
 */
export function Badge(props: {
    readonly tone: BadgeTone;
    readonly label: string;
    readonly title?: string | undefined;
}): JSX.Element {
    return (
        <span
            title={props.title}
            class={joinClass(
                "inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-[12.5px] font-medium whitespace-nowrap",
                BADGE_CLASS[props.tone],
            )}
        >
            <Glyph name={BADGE_GLYPH[props.tone]} />
            {props.label}
        </span>
    );
}

// The headline-figure typography `Stat` and `StatLink` share. `StatLink`'s
// inline `span`s append `block` where `Stat`'s `p`s are block-level already.
const STAT_VALUE_CLASS = "tnum truncate text-[26px] leading-none font-semibold tracking-tight";
const STAT_LABEL_CLASS = "mt-2 text-[13px] font-medium text-ink-dim";

/** One headline figure for the overview: a number with its name. */
export function Stat(props: {
    readonly value: string;
    readonly label: string;
    readonly hint?: string | undefined;
}): JSX.Element {
    return (
        <div class="min-w-0">
            <p class={STAT_VALUE_CLASS}>{props.value}</p>
            <p class={STAT_LABEL_CLASS}>{props.label}</p>
            <Show when={props.hint}>
                {(hint) => <p class="mt-0.5 text-[12.5px] text-ink-faint">{hint()}</p>}
            </Show>
        </div>
    );
}

/**
 * One headline figure that is also a door: the whole card is the link, so the
 * figure and the page that explains it are one target. The optional hint
 * qualifies the figure the same line `Stat` states its own — a figure the raw
 * count alone can misread states its scope under it. The chevron marks the
 * card as navigation — the same affordance a table row carries.
 */
export function StatLink(props: {
    readonly href: string;
    readonly value: string;
    readonly label: string;
    readonly hint?: string | undefined;
}): JSX.Element {
    return (
        <AppLink
            href={props.href}
            class="group flex items-start justify-between gap-3 rounded-2xl border border-rule bg-surface px-4 py-4 transition-colors duration-150 ease-out hover:border-ink-faint hover:bg-hover"
        >
            <span class="min-w-0">
                <span class={joinClass(STAT_VALUE_CLASS, "block")}>{props.value}</span>
                <span class={joinClass(STAT_LABEL_CLASS, "block")}>{props.label}</span>
                <Show when={props.hint}>
                    {(hint) => (
                        <span class="mt-0.5 block text-[12.5px] text-ink-faint">{hint()}</span>
                    )}
                </Show>
            </span>
            <Glyph
                name="chev-right"
                class="mt-1 shrink-0 text-ink-faint transition-colors duration-150 ease-out group-hover:text-accent"
            />
        </AppLink>
    );
}

/** A page with nothing to show yet: what this is, and the one way forward. */
export function EmptyState(props: {
    readonly glyph?: GlyphName | undefined;
    readonly title: JSX.Element;
    readonly description: JSX.Element;
    readonly children?: JSX.Element | undefined;
}): JSX.Element {
    return (
        <div class="flex flex-col items-start gap-3 rounded-2xl border border-rule bg-surface px-5 py-8 sm:px-8">
            <span class="grid size-10 place-items-center rounded-full bg-accent-soft text-accent dark:text-accent-strong">
                <Glyph name={props.glyph ?? "info"} />
            </span>
            <h2 class="text-[16px] font-semibold tracking-tight">{props.title}</h2>
            <p class="max-w-[70ch] text-ink-dim">{props.description}</p>
            <Show when={props.children}>
                {(actions) => <div class="mt-1 flex flex-wrap gap-2">{actions()}</div>}
            </Show>
        </div>
    );
}

/** Placeholder blocks while a screen's first read is in flight. */
export function Skeleton(props: { readonly class?: string | undefined }): JSX.Element {
    return (
        <div
            aria-hidden="true"
            class={joinClass("wash rounded-lg bg-muted", props.class ?? "h-4")}
        />
    );
}
