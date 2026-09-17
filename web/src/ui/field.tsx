import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

export type SelectOption = {
    readonly value: string;
    readonly label: string;
};

export type TextFieldProps = {
    readonly id: string;
    readonly label: string;
    readonly value: string;
    readonly onInput: (value: string) => void;
    readonly type?: "text" | "password";
    readonly placeholder?: string;
    readonly autocomplete?: string;
    readonly required?: boolean;
    readonly disabled?: boolean;
    readonly isMonospace?: boolean;
    readonly hint?: string | null;
    readonly error?: string | null;
};

export type TextAreaFieldProps = {
    readonly id: string;
    readonly label: string;
    readonly value: string;
    readonly onInput: (value: string) => void;
    readonly rows?: number;
    readonly placeholder?: string;
    readonly required?: boolean;
    readonly disabled?: boolean;
    readonly isMonospace?: boolean;
    readonly hint?: string | null;
    readonly error?: string | null;
};

export type SelectFieldProps = {
    readonly id: string;
    readonly label: string;
    readonly value: string;
    readonly options: ReadonlyArray<SelectOption>;
    readonly onChange: (value: string) => void;
    readonly disabled?: boolean;
    readonly hint?: string | null;
    readonly error?: string | null;
};

export type CheckboxFieldProps = {
    readonly id: string;
    readonly label: string;
    readonly checked: boolean;
    readonly onChange: (checked: boolean) => void;
    readonly disabled?: boolean;
    readonly hint?: string | null;
};

type FieldMessageProps = {
    readonly id: string;
    readonly hint: string | null | undefined;
    readonly error: string | null | undefined;
};

function FieldMessage(props: FieldMessageProps): JSX.Element {
    return (
        <>
            <Show when={props.error ?? null}>
                {(message) => (
                    <p
                        id={`${props.id}-error`}
                        class="text-xs text-red-600"
                    >
                        {message()}
                    </p>
                )}
            </Show>
            <Show when={props.hint ?? null}>
                {(hint) => (
                    <p
                        id={`${props.id}-hint`}
                        class="text-xs text-slate-500"
                    >
                        {hint()}
                    </p>
                )}
            </Show>
        </>
    );
}

function describedBy(props: {
    readonly id: string;
    readonly hint?: string | null;
    readonly error?: string | null;
}): string | undefined {
    const ids: string[] = [];
    if ((props.error ?? null) !== null) {
        ids.push(`${props.id}-error`);
    }
    if ((props.hint ?? null) !== null) {
        ids.push(`${props.id}-hint`);
    }
    return ids.length === 0 ? undefined : ids.join(" ");
}

const inputClass =
    "w-full rounded-md border px-3 py-2 text-sm text-slate-900 shadow-sm placeholder:text-slate-400 focus:ring-2 focus:ring-slate-200 focus:outline-none disabled:bg-slate-100";

export function TextField(props: TextFieldProps): JSX.Element {
    return (
        <div class="flex flex-col gap-1">
            <label
                for={props.id}
                class="text-sm font-medium text-slate-700"
            >
                {props.label}
            </label>
            <input
                id={props.id}
                type={props.type ?? "text"}
                class={[
                    inputClass,
                    {
                        "border-slate-300 bg-white focus:border-slate-500":
                            (props.error ?? null) === null,
                        "border-red-400 bg-white focus:border-red-500":
                            (props.error ?? null) !== null,
                        "font-mono": props.isMonospace ?? false,
                    },
                ]}
                value={props.value}
                placeholder={props.placeholder}
                autocomplete={props.autocomplete}
                required={props.required ?? false}
                disabled={props.disabled ?? false}
                aria-invalid={(props.error ?? null) !== null ? "true" : "false"}
                aria-describedby={describedBy(props)}
                onInput={(event) => props.onInput(event.currentTarget.value)}
            />
            <FieldMessage
                id={props.id}
                hint={props.hint}
                error={props.error}
            />
        </div>
    );
}

export function TextAreaField(props: TextAreaFieldProps): JSX.Element {
    return (
        <div class="flex flex-col gap-1">
            <label
                for={props.id}
                class="text-sm font-medium text-slate-700"
            >
                {props.label}
            </label>
            <textarea
                id={props.id}
                rows={props.rows ?? 3}
                class={[
                    inputClass,
                    {
                        "border-slate-300 bg-white focus:border-slate-500":
                            (props.error ?? null) === null,
                        "border-red-400 bg-white focus:border-red-500":
                            (props.error ?? null) !== null,
                        "font-mono": props.isMonospace ?? false,
                    },
                ]}
                value={props.value}
                placeholder={props.placeholder}
                required={props.required ?? false}
                disabled={props.disabled ?? false}
                aria-invalid={(props.error ?? null) !== null ? "true" : "false"}
                aria-describedby={describedBy(props)}
                onInput={(event) => props.onInput(event.currentTarget.value)}
            />
            <FieldMessage
                id={props.id}
                hint={props.hint}
                error={props.error}
            />
        </div>
    );
}

export function SelectField(props: SelectFieldProps): JSX.Element {
    return (
        <div class="flex flex-col gap-1">
            <label
                for={props.id}
                class="text-sm font-medium text-slate-700"
            >
                {props.label}
            </label>
            <select
                id={props.id}
                class={[
                    inputClass,
                    {
                        "border-slate-300 bg-white focus:border-slate-500":
                            (props.error ?? null) === null,
                        "border-red-400 bg-white focus:border-red-500":
                            (props.error ?? null) !== null,
                    },
                ]}
                value={props.value}
                disabled={props.disabled ?? false}
                aria-describedby={describedBy(props)}
                onChange={(event) => props.onChange(event.currentTarget.value)}
            >
                {props.options.map((option) => (
                    <option value={option.value}>{option.label}</option>
                ))}
            </select>
            <FieldMessage
                id={props.id}
                hint={props.hint}
                error={props.error}
            />
        </div>
    );
}

export function CheckboxField(props: CheckboxFieldProps): JSX.Element {
    return (
        <div class="flex flex-col gap-1">
            <label
                for={props.id}
                class="flex items-center gap-2 text-sm text-slate-700"
            >
                <input
                    id={props.id}
                    type="checkbox"
                    class="size-4 rounded border-slate-300 text-slate-900 focus:ring-2 focus:ring-slate-200"
                    checked={props.checked}
                    disabled={props.disabled ?? false}
                    onChange={(event) => props.onChange(event.currentTarget.checked)}
                />
                {props.label}
            </label>
            <Show when={props.hint ?? null}>
                {(hint) => (
                    <p
                        id={`${props.id}-hint`}
                        class="text-xs text-slate-500"
                    >
                        {hint()}
                    </p>
                )}
            </Show>
        </div>
    );
}
