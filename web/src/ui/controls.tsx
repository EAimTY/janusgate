import type { JSX } from "@solidjs/web";
import { createUniqueId, For, Show } from "solid-js";
import { AppLink } from "/src/ui/app-link";
import { ariaBoolean, joinClass } from "/src/ui/attributes";
import { Glyph, type GlyphName } from "/src/ui/glyph";

/** A labelled field: what an input is for, what went wrong, and a hint. */
export function FieldFrame(props: {
    readonly label: string;
    readonly controlId: string;
    readonly hint?: string | null | undefined;
    readonly error?: string | null | undefined;
    readonly children: JSX.Element;
    readonly class?: string | undefined;
}): JSX.Element {
    return (
        <div class={props.class}>
            <label
                for={props.controlId}
                class="block text-[13px] font-medium text-ink"
            >
                {props.label}
            </label>
            <div class="mt-1.5">{props.children}</div>
            <Show when={props.error}>
                {(error) => (
                    <p
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
                {(hint) => <p class="mt-1.5 max-w-[70ch] text-[13px] text-ink-faint">{hint()}</p>}
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
            <input
                id={controlId}
                type={props.type ?? "text"}
                class={joinClass(INPUT_CLASS, props.isData === false ? "" : "tnum")}
                value={props.value}
                placeholder={props.placeholder}
                disabled={props.isDisabled ?? false}
                autocomplete={props.autocomplete ?? "off"}
                spellcheck={false}
                onInput={(event) => props.onInput(event.currentTarget.value)}
            />
        </FieldFrame>
    );
}

export function SelectField<T extends string>(props: {
    readonly label: string;
    readonly value: T;
    readonly options: ReadonlyArray<{
        readonly value: T;
        readonly label: string;
    }>;
    readonly onChange: (value: T) => void;
    readonly hint?: string | null | undefined;
    readonly error?: string | null | undefined;
    readonly isDisabled?: boolean;
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
            <select
                id={controlId}
                class={joinClass(INPUT_CLASS, "tnum pr-8")}
                disabled={props.isDisabled ?? false}
                onChange={(event) => {
                    const selected = props.options.find(
                        (option) => option.value === event.currentTarget.value,
                    );

                    if (selected !== undefined) {
                        props.onChange(selected.value);
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
            </select>
        </FieldFrame>
    );
}

export type ButtonTone = "primary" | "secondary" | "danger" | "ghost";

const BUTTON_TONE_CLASS: Record<ButtonTone, string> = {
    primary: "bg-accent text-accent-ink hover:bg-accent-strong",
    secondary:
        "border border-rule-strong bg-surface text-ink hover:border-ink-faint hover:bg-hover",
    danger: "bg-bad text-white hover:brightness-95 dark:text-[#1a0b09]",
    ghost: "text-ink-dim hover:bg-hover hover:text-ink",
};

function buttonClass(tone: ButtonTone, className?: string): string {
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
    readonly class?: string | undefined;
    readonly onClick?: () => void;
}): JSX.Element {
    return (
        <button
            type={props.type ?? "button"}
            class={buttonClass(props.tone ?? "secondary", props.class)}
            disabled={(props.isDisabled ?? false) || (props.isBusy ?? false)}
            title={props.title}
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
    readonly isCurrent?: boolean;
    readonly title?: string | undefined;
    readonly class?: string | undefined;
}): JSX.Element {
    return (
        <AppLink
            href={props.href}
            isCurrent={props.isCurrent ?? false}
            class={buttonClass(props.tone ?? "secondary", props.class)}
        >
            <Show when={props.glyph}>{(name) => <Glyph name={name()} />}</Show>
            {props.children}
        </AppLink>
    );
}

/**
 * A labeled on/off switch. The label names the thing being switched and the
 * knob's position is mirrored by `aria-checked`, so the state is never
 * color alone.
 */
export function Switch(props: {
    readonly label: string;
    readonly hint?: string | null | undefined;
    readonly isOn: boolean;
    readonly onText?: string | undefined;
    readonly offText?: string | undefined;
    readonly isDisabled?: boolean;
    readonly onToggle: (isOn: boolean) => void;
    readonly class?: string | undefined;
}): JSX.Element {
    return (
        <div class={joinClass("flex items-start justify-between gap-4", props.class ?? "")}>
            <div class="min-w-0">
                <p class="text-[13px] font-medium text-ink">{props.label}</p>
                <Show when={props.hint}>
                    {(hint) => <p class="mt-1 max-w-[70ch] text-[13px] text-ink-faint">{hint()}</p>}
                </Show>
            </div>
            <button
                type="button"
                role="switch"
                aria-checked={ariaBoolean(props.isOn)}
                aria-label={props.label}
                disabled={props.isDisabled ?? false}
                class={joinClass(
                    "relative mt-0.5 h-6 w-11 shrink-0 cursor-pointer rounded-full transition-colors duration-150 ease-out disabled:cursor-not-allowed disabled:opacity-50",
                    props.isOn ? "bg-accent" : "bg-rule-strong dark:bg-muted",
                )}
                onClick={() => props.onToggle(!props.isOn)}
            >
                <span
                    aria-hidden="true"
                    class={joinClass(
                        "absolute top-0.5 left-0.5 block size-5 rounded-full bg-white shadow transition-transform duration-150 ease-out dark:bg-ink",
                        props.isOn ? "translate-x-5" : "translate-x-0",
                    )}
                />
            </button>
            <Show when={props.onText !== undefined || props.offText !== undefined}>
                <span class="mt-1 w-12 shrink-0 text-right text-[13px] font-medium text-ink-dim">
                    {props.isOn ? (props.onText ?? "On") : (props.offText ?? "Off")}
                </span>
            </Show>
        </div>
    );
}
