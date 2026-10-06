import type { JSX } from "@solidjs/web";
import { createEffect, createMemo, createUniqueId, For, Show } from "solid-js";
import { translation } from "/src/i18n";
import { AppLink } from "/src/ui/app-link";
import { ariaBoolean, joinClass } from "/src/ui/attributes";
import { Glyph, type GlyphName } from "/src/ui/glyph";

/**
 * A labelled field: what an input is for, what went wrong, and a hint. The
 * control receives the ids of the hint and error paragraphs through its
 * `describedBy` callback, so a focused field announces its standing error
 * and its hint instead of its name alone; the callback reads the current
 * hint and error, so the wiring follows them as they come and go.
 */
function FieldFrame(props: {
    readonly label: JSX.Element;
    readonly controlId: string;
    readonly hint?: string | null | undefined;
    readonly error?: string | null | undefined;
    readonly children: (describedBy: () => string | undefined) => JSX.Element;
    readonly class?: string | undefined;
}): JSX.Element {
    const hintId = createUniqueId();
    const errorId = createUniqueId();

    // The standing error's id leads — an alert is announced ahead of the
    // field's own text — then the hint's. With neither showing, the control
    // describes nothing.
    const describedBy = (): string | undefined => {
        const ids = [
            ...(props.error
                ? [
                      errorId,
                  ]
                : []),
            ...(props.hint
                ? [
                      hintId,
                  ]
                : []),
        ];

        return ids.length === 0 ? undefined : ids.join(" ");
    };

    return (
        <div class={props.class}>
            <label
                for={props.controlId}
                class="block text-[13px] font-medium text-ink"
            >
                {props.label}
            </label>
            <div class="mt-1.5">{props.children(describedBy)}</div>
            <Show when={props.error}>
                {(error) => (
                    <p
                        id={errorId}
                        class="mt-1.5 flex items-start gap-1.5 text-[13px] text-bad"
                        role="alert"
                    >
                        <Glyph
                            name="alert"
                            class="mt-0.5 shrink-0"
                        />
                        {error()}
                    </p>
                )}
            </Show>
            <Show when={props.hint}>
                {(hint) => (
                    <p
                        id={hintId}
                        class="mt-1.5 max-w-[70ch] text-[13px] text-ink-faint"
                    >
                        {hint()}
                    </p>
                )}
            </Show>
        </div>
    );
}

const INPUT_CLASS =
    "w-full rounded-lg border border-rule-strong bg-canvas px-3 py-2 text-ink placeholder:text-ink-faint transition-colors duration-150 ease-out hover:border-ink-faint focus:border-accent focus:outline-none disabled:cursor-not-allowed disabled:opacity-60 dark:bg-muted";

export function TextField(props: {
    readonly label: string;
    readonly value: string;
    readonly onInput: (value: string) => void;
    readonly type?: "text" | "password" | "url";
    readonly placeholder?: string | undefined;
    readonly hint?: string | null | undefined;
    readonly error?: string | null | undefined;
    readonly isDisabled?: boolean;
    readonly isData?: boolean;
    readonly autocomplete?: string | undefined;
    readonly class?: string | undefined;
}): JSX.Element {
    const controlId = createUniqueId();

    return (
        <FieldFrame
            label={props.label}
            controlId={controlId}
            hint={props.hint}
            error={props.error}
            class={props.class}
        >
            {(describedBy) => (
                <input
                    id={controlId}
                    type={props.type ?? "text"}
                    dir={props.isData === false ? "auto" : "ltr"}
                    class={joinClass(INPUT_CLASS, props.isData === false ? "" : "tnum")}
                    value={props.value}
                    placeholder={props.placeholder}
                    disabled={props.isDisabled ?? false}
                    autocomplete={props.autocomplete ?? "off"}
                    spellcheck={false}
                    aria-describedby={describedBy()}
                    onInput={(event) => props.onInput(event.currentTarget.value)}
                />
            )}
        </FieldFrame>
    );
}

/**
 * The label of the trailing option a select renders for a value none of its
 * options carry: the id the entity listings, once read, no longer hold.
 */
export function noLongerListedLabel(value: string): string {
    return translation()(($) => $.domainMessages.unlisted, {
        value,
    });
}

export function SelectField<T extends string>(props: {
    readonly label: JSX.Element;
    readonly value: T;
    readonly options: ReadonlyArray<{
        readonly value: T;
        readonly label: JSX.Element;
    }>;
    /**
     * The label of the trailing option rendered when no offered option
     * carries `value`, so the select keeps stating the choice in force
     * instead of silently resetting. Absent: the value itself.
     */
    readonly absentLabel?: ((value: T) => string) | undefined;
    readonly onChange: (value: T) => void;
    readonly hint?: string | null | undefined;
    readonly isDisabled?: boolean;
    readonly class?: string | undefined;
}): JSX.Element {
    const controlId = createUniqueId();

    // A value the options do not carry still selects something: one trailing
    // option states it, worded by `absentLabel` where the caller can name the
    // absence. The listed options ride along untouched, so only this option's
    // node turns over when the absent value moves.
    const absentOption = createMemo(() => {
        const value = props.value;

        if (props.options.some((option) => option.value === value)) {
            return null;
        }

        return {
            value,
            label: props.absentLabel?.(value) ?? value,
        };
    });

    let selectElement: HTMLSelectElement | undefined;

    // The displayed selection is the select's own state, driven from the
    // value: this re-asserts it whenever the value moves, so the select
    // follows its value even when no option node is recreated. The effect
    // runs after the flush's DOM updates, so the assignment always lands on
    // the options the flush produced; the options' `selected` attributes
    // carry the first paint until it runs.
    createEffect(
        () => props.value,
        (value) => {
            if (selectElement !== undefined) {
                selectElement.value = value;
            }
        },
    );

    return (
        <FieldFrame
            label={props.label}
            controlId={controlId}
            hint={props.hint}
            class={props.class}
        >
            {(describedBy) => (
                <select
                    ref={(element) => {
                        selectElement = element;
                    }}
                    id={controlId}
                    class={joinClass(INPUT_CLASS, "tnum pe-8")}
                    disabled={props.isDisabled ?? false}
                    aria-describedby={describedBy()}
                    onChange={(event) => {
                        const picked = event.currentTarget.value;
                        const selected = props.options.find((option) => option.value === picked);

                        if (selected !== undefined) {
                            props.onChange(selected.value);
                        } else if (picked === props.value) {
                            // The trailing absent-value option: the choice the
                            // caller already holds, reported like any other.
                            props.onChange(props.value);
                        }
                    }}
                >
                    <For each={props.options}>
                        {(option) => (
                            <option
                                value={option.value}
                                selected={option.value === props.value}
                            >
                                {option.label}
                            </option>
                        )}
                    </For>
                    <Show when={absentOption()}>
                        {(absent) => (
                            <option
                                value={absent().value}
                                selected={true}
                            >
                                {absent().label}
                            </option>
                        )}
                    </Show>
                </select>
            )}
        </FieldFrame>
    );
}

/**
 * The options an entity select offers: the absent head entry — the
 * "everything" choice every entity select opens with — followed by one option
 * per row. A row's option value is its id as an exact decimal string
 * (`String(row.id)`): the spelling the route seeds, the saved filters, and
 * the id comparisons all read and write, so ids must ride the options
 * verbatim, never as numbers or padded text. The label is each row's own,
 * named by `labelOf`; the head entry's wording is the caller's `absentLabel`.
 */
export function entitySelectOptions<
    TRow extends {
        readonly id: number;
    },
>(
    rows: ReadonlyArray<TRow>,
    labelOf: (row: TRow) => string,
    absentLabel: string,
): ReadonlyArray<{
    readonly value: string;
    readonly label: string;
}> {
    return [
        {
            value: "",
            label: absentLabel,
        },
        ...rows.map((row) => ({
            value: String(row.id),
            label: labelOf(row),
        })),
    ];
}

/**
 * The glyph-wrapped search field a listing narrows with. The `label` is the
 * field's accessible name; no listing search renders a visible label.
 */
export function SearchInput(props: {
    readonly label: string;
    readonly value: string;
    readonly onInput: (value: string) => void;
    readonly placeholder: string;
    readonly class?: string | undefined;
}): JSX.Element {
    return (
        <div class={props.class}>
            <div class="relative">
                <Glyph
                    name="search"
                    class="pointer-events-none absolute top-1/2 start-3 -translate-y-1/2 text-ink-faint"
                />
                <input
                    type="search"
                    dir="auto"
                    aria-label={props.label}
                    placeholder={props.placeholder}
                    value={props.value}
                    onInput={(event) => props.onInput(event.currentTarget.value)}
                    class={joinClass(INPUT_CLASS, "ps-9")}
                />
            </div>
        </div>
    );
}

/**
 * The needle a listing's search box narrows by: the search value trimmed and
 * case-folded for the listing's per-field `.includes` match. An empty needle
 * narrows nothing — the listing shows everything.
 */
export function listingNeedle(value: string): string {
    return value.trim().toLowerCase();
}

type ButtonTone = "primary" | "secondary" | "danger" | "ghost";

const BUTTON_TONE_CLASS: Record<ButtonTone, string> = {
    primary: "bg-accent text-accent-ink hover:bg-accent-strong",
    secondary:
        "border border-rule-strong bg-surface text-ink hover:border-ink-faint hover:bg-hover",
    danger: "bg-bad text-bad-ink hover:brightness-95",
    ghost: "text-ink-dim hover:bg-hover hover:text-ink",
};

/**
 * The composed class of one button tone — the shared visual grammar for
 * elements `Button` and `ButtonLink` cannot carry, such as an anchor to an
 * external page.
 */
export function buttonClass(tone: ButtonTone, className?: string): string {
    return joinClass(
        "inline-flex cursor-pointer items-center justify-center gap-2 rounded-lg px-3.5 py-2 text-[13.5px] font-medium transition-colors duration-150 ease-out disabled:cursor-not-allowed disabled:opacity-50",
        BUTTON_TONE_CLASS[tone],
        className ?? "",
    );
}

/** A button. Its label names the action; the tone names its weight. */
export function Button(props: {
    readonly children: JSX.Element;
    readonly tone?: ButtonTone;
    readonly glyph?: GlyphName | undefined;
    readonly type?: "button" | "submit";
    readonly isDisabled?: boolean;
    readonly isBusy?: boolean;
    readonly title?: string | undefined;
    /** The button's accessible name, where its visible label is shared by several buttons. */
    readonly ariaLabel?: string | undefined;
    readonly class?: string | undefined;
    readonly onClick?: () => void;
}): JSX.Element {
    return (
        <button
            type={props.type ?? "button"}
            class={buttonClass(props.tone ?? "secondary", props.class)}
            disabled={(props.isDisabled ?? false) || (props.isBusy ?? false)}
            title={props.title}
            aria-label={props.ariaLabel}
            onClick={() => props.onClick?.()}
        >
            <Show when={props.glyph}>{(name) => <Glyph name={name()} />}</Show>
            {props.children}
        </button>
    );
}

/** The same button, for in-app navigation. */
export function ButtonLink(props: {
    readonly href: string;
    readonly children: JSX.Element;
    readonly tone?: ButtonTone;
    readonly glyph?: GlyphName | undefined;
    readonly class?: string | undefined;
}): JSX.Element {
    return (
        <AppLink
            href={props.href}
            class={buttonClass(props.tone ?? "secondary", props.class)}
        >
            <Show when={props.glyph}>{(name) => <Glyph name={name()} />}</Show>
            {props.children}
        </AppLink>
    );
}

/**
 * A labeled on/off switch. The switch sits next to its label on the left
 * and the knob's position is mirrored by `aria-checked`, so the state is
 * never color alone. The label is a real `<label>` paired to the switch
 * through `for`, so clicking the visible text toggles exactly as clicking
 * the knob does — natively inert while the switch is disabled. The hint,
 * when present, states the current consequence of the switch's position,
 * and a focused switch announces it through `aria-describedby`.
 */
export function Switch(props: {
    readonly label: string;
    readonly hint?: string | null | undefined;
    readonly isOn: boolean;
    readonly isDisabled?: boolean;
    readonly onToggle: (isOn: boolean) => void;
    readonly class?: string | undefined;
}): JSX.Element {
    const controlId = createUniqueId();
    const hintId = createUniqueId();

    return (
        <div class={props.class}>
            <div class="flex items-center gap-2.5">
                <label
                    for={controlId}
                    class="text-[13px] font-medium text-ink"
                >
                    {props.label}
                </label>
                <button
                    id={controlId}
                    type="button"
                    role="switch"
                    aria-checked={ariaBoolean(props.isOn)}
                    aria-label={props.label}
                    aria-describedby={props.hint ? hintId : undefined}
                    disabled={props.isDisabled ?? false}
                    class={joinClass(
                        "relative h-6 w-11 shrink-0 cursor-pointer rounded-full transition-colors duration-150 ease-out disabled:cursor-not-allowed disabled:opacity-50",
                        props.isOn ? "bg-accent" : "bg-rule-strong dark:bg-muted",
                    )}
                    onClick={() => props.onToggle(!props.isOn)}
                >
                    <span
                        aria-hidden="true"
                        class={joinClass(
                            "absolute top-0.5 start-0.5 block size-5 rounded-full bg-surface transition-transform duration-150 ease-out dark:bg-ink",
                            props.isOn ? "translate-x-5 rtl:-translate-x-5" : "translate-x-0",
                        )}
                    />
                </button>
            </div>
            <Show when={props.hint}>
                {(hint) => (
                    <p
                        id={hintId}
                        class="mt-1.5 max-w-[70ch] text-[13px] text-ink-faint"
                    >
                        {hint()}
                    </p>
                )}
            </Show>
        </div>
    );
}
