import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";
import type { ProviderPreset } from "/src/management-api/presets";
import { ariaBoolean, joinClass } from "/src/ui/attributes";
import { Badge } from "/src/ui/feedback";
import { Switch, TextField } from "/src/ui/controls";

/**
 * The preset picker: one card per catalog entry, the static-key compatible
 * APIs first and the OAuth products after. A preset prefills the definition
 * the creation flow registers; the operator edits the JSON it produced, the
 * picker is only the starting point.
 */
export function ProviderPresetPicker(props: {
    readonly presets: ReadonlyArray<ProviderPreset>;
    readonly value: string;
    readonly onChange: (id: string) => void;
    readonly isDisabled?: boolean;
}): JSX.Element {
    return (
        <div class="grid gap-2 sm:grid-cols-2">
            <For each={props.presets}>
                {(preset) => {
                    const isSelected = (): boolean => props.value === preset.id;

                    return (
                        <button
                            type="button"
                            aria-pressed={ariaBoolean(isSelected())}
                            disabled={props.isDisabled ?? false}
                            onClick={() => props.onChange(preset.id)}
                            class={joinClass(
                                "cursor-pointer rounded-xl border px-3.5 py-3 text-left transition-colors duration-150 ease-out disabled:cursor-not-allowed disabled:opacity-60",
                                isSelected()
                                    ? "border-accent bg-accent-soft"
                                    : "border-rule-strong bg-surface hover:border-ink-faint",
                            )}
                        >
                            <span class="flex items-center gap-2">
                                <span class="block text-[13.5px] font-semibold">{preset.title}</span>
                                <Badge
                                    tone={preset.verification === "live" ? "ok" : "muted"}
                                    label={preset.verification === "live" ? "Verified live" : "Unverified"}
                                />
                            </span>
                            <span class="mt-1 block text-[12.5px] leading-snug text-ink-dim">
                                {preset.description}
                            </span>
                        </button>
                    );
                }}
            </For>
        </div>
    );
}

/**
 * Name and model prefix: the two fields every provider carries. The prefix
 * defaults to the provider's name plus one trailing dash, so an unoverridden
 * model reads as `name-upstream-id`; flipping the switch on exposes the field
 * for a hand-set value — including an empty one — which is then used exactly
 * as typed. A prefix never holds a slash or a colon: it is a routing token,
 * and the Gemini URL path cannot address either.
 */
export function ProviderIdentityFields(props: {
    readonly name: string;
    readonly namePlaceholder: string;
    readonly onNameInput: (value: string) => void;
    readonly nameError?: string | null | undefined;
    readonly isCustomPrefix: boolean;
    readonly onCustomPrefixToggle: (isCustom: boolean) => void;
    readonly customPrefix: string;
    readonly onCustomPrefixInput: (value: string) => void;
    readonly defaultPrefix: string;
    readonly prefixError?: string | null | undefined;
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
                error={props.nameError}
            />
            <Switch
                label="Custom model prefix"
                hint={
                    props.isCustomPrefix
                        ? "Models are served under the custom prefix below."
                        : `Models are served as ${props.defaultPrefix === "" ? "upstream-id" : `${props.defaultPrefix}upstream-id`}.`
                }
                isOn={props.isCustomPrefix}
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
                    error={props.prefixError}
                    hint="Used exactly as typed — no separator is added."
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
    readonly baseUrlError?: string | null | undefined;
    readonly apiKeyError?: string | null | undefined;
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
                error={props.baseUrlError}
            />
            <TextField
                label={props.apiKeyLabel ?? "API key"}
                type="password"
                value={props.apiKey}
                onInput={props.onApiKeyInput}
                isDisabled={props.isDisabled ?? false}
                autocomplete="off"
                error={props.apiKeyError}
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
            isDisabled={props.isDisabled ?? false}
            onToggle={props.onToggle}
        />
    );
}
