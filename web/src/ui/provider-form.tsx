import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";
import type { ProviderType } from "/src/management-api/generated/v0";
import { ariaBoolean, joinClass } from "/src/ui/attributes";
import { Switch, TextField } from "/src/ui/controls";

/** The five creatable provider kinds, exactly the wire contract's five provider types. */
export type ProviderKind = ProviderType;

const PROVIDER_KIND_TITLES: Record<ProviderKind, string> = {
    openai_response: "OpenAI Responses",
    openai_chat_completion: "OpenAI Chat Completions",
    gemini_generate_content: "Gemini generateContent",
    codex_oauth: "Codex OAuth",
    antigravity_oauth: "Antigravity OAuth",
};

const PROVIDER_KIND_DESCRIPTIONS: Record<ProviderKind, string> = {
    openai_response: "Any Responses API with a static key.",
    openai_chat_completion: "Any Chat Completions API with a static key.",
    gemini_generate_content: "Any generateContent API with a static key.",
    codex_oauth: "A ChatGPT account via its login page.",
    antigravity_oauth: "A Google account via its login page.",
};

/** The kind picker groups wire formats by family: OAuth logins, OpenAI-compatible keys, Gemini. */
const PROVIDER_KIND_GROUPS: ReadonlyArray<{
    readonly title: string;
    readonly kinds: ReadonlyArray<ProviderKind>;
}> = [
    {
        title: "OAuth",
        kinds: [
            "codex_oauth",
            "antigravity_oauth",
        ],
    },
    {
        title: "OpenAI-compatible",
        kinds: [
            "openai_response",
            "openai_chat_completion",
        ],
    },
    {
        title: "Gemini",
        kinds: [
            "gemini_generate_content",
        ],
    },
];

/**
 * The provider kind as grouped options. The three static-key wire formats
 * and the two OAuth logins are one choice here — not different creations —
 * because they are all providers whose credential paths merely differ.
 */
export function ProviderKindPicker(props: {
    readonly value: ProviderKind;
    readonly onChange: (value: ProviderKind) => void;
    readonly isDisabled?: boolean;
}): JSX.Element {
    return (
        <div class="flex flex-col gap-4">
            <For each={PROVIDER_KIND_GROUPS}>
                {(group) => (
                    <fieldset>
                        <legend class="text-[13px] font-medium text-ink-dim">{group.title}</legend>
                        <div class="mt-1.5 grid gap-2 sm:grid-cols-2">
                            <For each={group.kinds}>
                                {(kind) => {
                                    const isSelected = (): boolean => props.value === kind;

                                    return (
                                        <button
                                            type="button"
                                            aria-pressed={ariaBoolean(isSelected())}
                                            disabled={props.isDisabled ?? false}
                                            onClick={() => props.onChange(kind)}
                                            class={joinClass(
                                                "cursor-pointer rounded-xl border px-3.5 py-3 text-left transition-colors duration-150 ease-out disabled:cursor-not-allowed disabled:opacity-60",
                                                isSelected()
                                                    ? "border-accent bg-accent-soft"
                                                    : "border-rule-strong bg-surface hover:border-ink-faint",
                                            )}
                                        >
                                            <span class="block text-[13.5px] font-semibold">
                                                {PROVIDER_KIND_TITLES[kind]}
                                            </span>
                                            <span class="mt-1 block text-[12.5px] leading-snug text-ink-dim">
                                                {PROVIDER_KIND_DESCRIPTIONS[kind]}
                                            </span>
                                        </button>
                                    );
                                }}
                            </For>
                        </div>
                    </fieldset>
                )}
            </For>
        </div>
    );
}

/**
 * Name and model prefix: the two fields every provider carries. The prefix
 * defaults to the name with a slash; flipping the switch on exposes the
 * field for a hand-set value — including an empty one — which is then used
 * exactly as typed, with no slash added.
 */
export function ProviderIdentityFields(props: {
    readonly name: string;
    readonly namePlaceholder: string;
    readonly onNameInput: (value: string) => void;
    readonly isCustomPrefix: boolean;
    readonly onCustomPrefixToggle: (isCustom: boolean) => void;
    readonly customPrefix: string;
    readonly onCustomPrefixInput: (value: string) => void;
    readonly defaultPrefix: string;
    readonly isDisabled?: boolean;
}): JSX.Element {
    return (
        <div class="flex flex-col gap-4">
            <TextField
                label="Name"
                value={props.name}
                onInput={props.onNameInput}
                placeholder={props.namePlaceholder}
                isDisabled={props.isDisabled ?? false}
                isData={false}
            />
            <Switch
                label="Custom model prefix"
                hint={
                    props.isCustomPrefix
                        ? "On: the prefix below is used exactly as typed."
                        : `Off: models are served as ${props.defaultPrefix === "" ? "upstream-id" : `${props.defaultPrefix}upstream-id`}.`
                }
                isOn={props.isCustomPrefix}
                onText="On"
                offText="Off"
                isDisabled={props.isDisabled ?? false}
                onToggle={props.onCustomPrefixToggle}
            />
            <Show when={props.isCustomPrefix}>
                <TextField
                    label="Model prefix"
                    value={props.customPrefix}
                    onInput={props.onCustomPrefixInput}
                    placeholder="Empty for none"
                    isDisabled={props.isDisabled ?? false}
                    hint='Used exactly as typed — no "/" is added.'
                />
            </Show>
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
                    "Sent upstream with every request. Never returned by the gateway."
                }
            />
        </div>
    );
}

/** The one switch that stops or resumes everything on a provider. It always
 * reads `Enabled` with no caption, and it always stands first in the
 * settings it belongs to. */
export function ProviderEnabledSwitch(props: {
    readonly isEnabled: boolean;
    readonly onToggle: (isEnabled: boolean) => void;
    readonly isDisabled?: boolean;
}): JSX.Element {
    return (
        <Switch
            label="Enabled"
            isOn={props.isEnabled}
            onText="On"
            offText="Off"
            isDisabled={props.isDisabled ?? false}
            onToggle={props.onToggle}
        />
    );
}
