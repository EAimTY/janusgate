import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";
import { AppLink } from "/src/ui/app-link";
import { joinClass } from "/src/ui/attributes";
import { Glyph } from "/src/ui/glyph";

/** One content card: a white surface on the canvas, one border, no shadow. */
export function Card(props: {
    readonly children: JSX.Element;
    readonly class?: string | undefined;
}): JSX.Element {
    return (
        <section
            class={joinClass(
                "rounded-2xl border border-rule bg-surface px-5 py-5",
                props.class ?? "",
            )}
        >
            {props.children}
        </section>
    );
}

/** A detail card's fact grid: label-and-answer pairs over two columns from `sm` up. */
export function FactList(props: { readonly children: JSX.Element }): JSX.Element {
    return <dl class="mt-4 grid gap-x-8 gap-y-3 sm:grid-cols-2">{props.children}</dl>;
}

/**
 * One fact in a `FactList`: the term naming it and the answer beneath.
 * `isTabular` sets the answer in tabular numerals — timestamps, counts, and
 * other aligned figures — and `class` carries the answer's own extras, such
 * as `break-all` for a long identifier or `text-ink-dim` for a subdued one.
 */
export function Fact(props: {
    readonly label: string;
    readonly value: JSX.Element;
    readonly isTabular?: boolean;
    readonly class?: string | undefined;
}): JSX.Element {
    return (
        <div>
            <dt class="text-[12.5px] font-medium text-ink-dim">{props.label}</dt>
            <dd
                class={joinClass(props.isTabular === true ? "tnum" : "", "mt-1", props.class ?? "")}
            >
                {props.value}
            </dd>
        </div>
    );
}

/** A card's own heading row: title plus the actions that belong to it. */
export function CardHeader(props: {
    readonly title: string;
    readonly description?: string | undefined;
    readonly children?: JSX.Element | undefined;
}): JSX.Element {
    return (
        <div class="flex flex-wrap items-start justify-between gap-x-4 gap-y-3">
            <div class="min-w-0">
                <h2 class="text-[15px] font-semibold tracking-tight">{props.title}</h2>
                <Show when={props.description}>
                    {(description) => (
                        <p class="mt-1 max-w-[70ch] text-[13px] text-ink-dim">{description()}</p>
                    )}
                </Show>
            </div>
            <Show when={props.children}>
                {(actions) => <div class="flex flex-wrap items-center gap-2">{actions()}</div>}
            </Show>
        </div>
    );
}

/** A screen's heading: back link, title, description, and primary actions. */
export function PageHeader(props: {
    readonly title: string;
    readonly description?: string | undefined;
    readonly backHref?: string | undefined;
    readonly backLabel?: string | undefined;
    readonly children?: JSX.Element | undefined;
}): JSX.Element {
    return (
        <div class="flex flex-col gap-3">
            <Show when={props.backHref}>
                {(href) => (
                    <AppLink
                        href={href()}
                        class="inline-flex w-fit items-center gap-1 text-[13px] font-medium text-ink-dim transition-colors duration-150 ease-out hover:text-ink"
                    >
                        <Glyph name="chev-left" />
                        {props.backLabel ?? "Back"}
                    </AppLink>
                )}
            </Show>
            <div class="flex flex-wrap items-end justify-between gap-x-4 gap-y-3">
                <div class="min-w-0">
                    <h1 class="text-[20px] font-semibold tracking-tight text-balance">
                        {props.title}
                    </h1>
                    <Show when={props.description}>
                        {(description) => (
                            <p class="mt-1 max-w-[75ch] text-ink-dim">{description()}</p>
                        )}
                    </Show>
                </div>
                <Show when={props.children}>
                    {(actions) => <div class="flex flex-wrap items-center gap-2">{actions()}</div>}
                </Show>
            </div>
        </div>
    );
}
