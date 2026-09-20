import type { JSX } from "@solidjs/web";
import { For } from "solid-js";
import type { ProviderType } from "/src/management-api/generated/v0";
import { PROVIDER_TYPE_OPTIONS } from "/src/management-api/vocabulary";
import { ariaBoolean, joinClass } from "/src/ui/attributes";
import { Switch, TextField } from "/src/ui/controls";

/** The five creatable provider kinds, exactly the wire contract's five provider types. */
export type ProviderKind = ProviderType;

const PROVIDER_KIND_DESCRIPTIONS: Record<ProviderKind, string> = {
    openai_response:
        "Any service speaking the OpenAI Responses API, authenticated with a static key.",
    openai_chat_completion:
        "Any service speaking the OpenAI Chat Completions API, authenticated with a static key.",
    gemini_generate_content:
        "Any service speaking the Gemini generateContent API, authenticated with a static key.",
    codex_oauth: "A ChatGPT account, connected with Codex OAuth through its login page.",
    antigravity_oauth: "A Google account, connected with Antigravity OAuth through its login page.",
};

export const PROVIDER_KIND_OPTIONS: ReadonlyArray<{
    readonly value: ProviderKind;
    readonly title: string;
    readonly description: string;
}> = PROVIDER_TYPE_OPTIONS.map((option) => ({
    value: option.value,
    title: option.label,
    description: PROVIDER_KIND_DESCRIPTIONS[option.value],
}));

/**
 * The provider kind as five plain options. The three static-key wire formats
 * and the two OAuth logins are one choice here — not different creations —
 * because they are all providers whose credential paths merely differ.
 */
export function ProviderKindPicker(props: {
    readonly value: ProviderKind;
    readonly onChange: (value: ProviderKind) => void;
    readonly isDisabled?: boolean;
}): JSX.Element {
    return (
        <fieldset>
            <legend class="sr-only">Provider type</legend>
            <div class="grid gap-2 sm:grid-cols-3">
                <For each={PROVIDER_KIND_OPTIONS}>
                    {(option) => {
                        const isSelected = (): boolean => props.value === option.value;

                        return (
                            <button
                                type="button"
                                aria-pressed={ariaBoolean(isSelected())}
                                disabled={props.isDisabled ?? false}
                                onClick={() => props.onChange(option.value)}
                                class={joinClass(
                                    "cursor-pointer rounded-xl border px-3.5 py-3 text-left transition-colors duration-150 ease-out disabled:cursor-not-allowed disabled:opacity-60",
                                    isSelected()
                                        ? "border-accent bg-accent-soft"
                                        : "border-rule-strong bg-surface hover:border-ink-faint",
                                )}
                            >
                                <span class="block text-[13.5px] font-semibold">
                                    {option.title}
                                </span>
                                <span class="mt-1 block text-[12.5px] leading-snug text-ink-dim">
                                    {option.description}
                                </span>
                            </button>
                        );
                    }}
                </For>
            </div>
        </fieldset>
    );
}

/** Name and model prefix: the two fields every provider carries. */
export function ProviderIdentityFields(props: {
    readonly name: string;
    readonly modelPrefix: string;
    readonly onNameInput: (value: string) => void;
    readonly onModelPrefixInput: (value: string) => void;
    readonly isDisabled?: boolean;
}): JSX.Element {
    return (
        <div class="grid gap-4 sm:grid-cols-2">
            <TextField
                label="Name"
                value={props.name}
                onInput={props.onNameInput}
                isDisabled={props.isDisabled ?? false}
                isData={false}
                hint="The name clients see as the model's owner, and the name a conflict is reported against."
            />
            <TextField
                label="Model prefix"
                value={props.modelPrefix}
                onInput={props.onModelPrefixInput}
                isDisabled={props.isDisabled ?? false}
                hint="Placed before every upstream ID: prefix + upstream ID is the public ID, unless a model overrides it."
            />
        </div>
    );
}

/**
 * Address and key of a static-key provider. The wire format is the
 * provider's own type and never an editable field, so this card carries no
 * format control.
 */
export function CompatibleConnectionFields(props: {
    readonly baseUrl: string;
    readonly apiKey: string;
    readonly onBaseUrlInput: (value: string) => void;
    readonly onApiKeyInput: (value: string) => void;
    readonly apiKeyLabel?: string | undefined;
    readonly apiKeyHint?: string | undefined;
    readonly isDisabled?: boolean;
}): JSX.Element {
    return (
        <div class="flex flex-col gap-4">
            <TextField
                label="Upstream base URL"
                type="url"
                value={props.baseUrl}
                onInput={props.onBaseUrlInput}
                placeholder="https://upstream.example.com/v1"
                isDisabled={props.isDisabled ?? false}
                hint="Where this provider's traffic is sent."
            />
            <TextField
                label={props.apiKeyLabel ?? "API key"}
                type="password"
                value={props.apiKey}
                onInput={props.onApiKeyInput}
                isDisabled={props.isDisabled ?? false}
                autocomplete="off"
                hint={
                    props.apiKeyHint ??
                    "Sent upstream with every request. The gateway stores it and never returns it."
                }
            />
        </div>
    );
}

/** The one switch that stops or resumes everything on a provider. */
export function ProviderServingSwitch(props: {
    readonly isEnabled: boolean;
    readonly onToggle: (isEnabled: boolean) => void;
    readonly isDisabled?: boolean;
}): JSX.Element {
    return (
        <Switch
            label="Serving"
            hint="Turn off to stop serving every model on this provider. A model serves only while both its own switch and this one are on."
            isOn={props.isEnabled}
            onText="On"
            offText="Off"
            isDisabled={props.isDisabled ?? false}
            onToggle={props.onToggle}
        />
    );
}
