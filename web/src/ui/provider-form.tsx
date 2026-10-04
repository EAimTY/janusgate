import type { JSX } from "@solidjs/web";
import { createSignal, For, Show } from "solid-js";
import type { ProviderPreset } from "/src/presets";
import { ariaBoolean, joinClass } from "/src/ui/attributes";
import { Switch, TextField } from "/src/ui/controls";
import { Badge } from "/src/ui/feedback";
import { requiredHint } from "./required-hint";
import { SecretRevealToggle } from "./secret-reveal";

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
                                <span class="block text-[13.5px] font-semibold">
                                    {preset.title}
                                </span>
                                <Badge
                                    tone={preset.verification === "live" ? "ok" : "muted"}
                                    label={
                                        preset.verification === "live"
                                            ? "Verified live"
                                            : "Unverified"
                                    }
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

/** The account's free-form management label, independent of public model routes. */
export function ProviderIdentityFields(props: {
    readonly name: string;
    readonly namePlaceholder: string;
    readonly modelPrefix: string;
    readonly onModelPrefixInput: (value: string) => void;
    readonly prefixError?: string | null | undefined;
    readonly onNameInput: (value: string) => void;
    readonly nameError?: string | null | undefined;
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
            <TextField
                label="Model prefix"
                value={props.modelPrefix}
                onInput={props.onModelPrefixInput}
                isDisabled={props.isDisabled ?? false}
                error={props.prefixError}
                hint="Outside pools, models are callable as this prefix + upstream ID. Members use their pool prefixes. Leave empty to use upstream IDs directly. Changing the prefix changes existing automatic model names."
            />
        </div>
    );
}

/**
 * Address and key of a static-key provider. The wire format is the
 * provider's own type and never an editable field, so this card carries no
 * format control. The base URL is part of the connection only where the
 * definition's URLs reference the base — a definition that pins its
 * endpoints, or mints the base from its flow, creates with `null` and shows
 * no field for one (`hasBaseUrl` defaults to shown, the shape every
 * definition-bound row renders). An explicit `true` is the creation shape:
 * there the submit gate demands the address, and the field states that
 * statically; the default shape shows the same field prefilled on a row
 * (the edit form), where no pristine gate waits on it. The API key's default
 * hint states the creation contract, where the gate requires a key — a
 * caller stating its own hint (the edit form's write-only contract) owns the
 * whole line. The key is masked like every pasted secret and carries the
 * connection screen's reveal, so an operator can confirm a paste here instead
 * of meeting a mangled key later as a discovery or upstream failure.
 */
export function CompatibleConnectionFields(props: {
    readonly baseUrl: string;
    readonly apiKey: string;
    readonly hasBaseUrl?: boolean;
    readonly onBaseUrlInput: (value: string) => void;
    readonly onApiKeyInput: (value: string) => void;
    readonly apiKeyLabel?: string | undefined;
    readonly apiKeyHint?: string | undefined;
    readonly baseUrlError?: string | null | undefined;
    readonly apiKeyError?: string | null | undefined;
    readonly isDisabled?: boolean;
}): JSX.Element {
    // The reveal is the field's own transient state: every mount opens masked.
    const [isKeyVisible, setIsKeyVisible] = createSignal(false);

    return (
        <div class="flex flex-col gap-4">
            <Show when={props.hasBaseUrl !== false}>
                <TextField
                    label="Upstream base URL"
                    type="url"
                    value={props.baseUrl}
                    onInput={props.onBaseUrlInput}
                    placeholder="https://upstream.example.com/v1"
                    isDisabled={props.isDisabled ?? false}
                    error={props.baseUrlError}
                    hint={
                        props.hasBaseUrl === true
                            ? requiredHint(
                                  "The upstream address this definition's requests point at.",
                              )
                            : undefined
                    }
                />
            </Show>
            <TextField
                label={props.apiKeyLabel ?? "Upstream API key"}
                type={isKeyVisible() ? "text" : "password"}
                value={props.apiKey}
                onInput={props.onApiKeyInput}
                isDisabled={props.isDisabled ?? false}
                autocomplete="off"
                error={props.apiKeyError}
                hint={
                    props.apiKeyHint ??
                    requiredHint("Sent upstream with every request. Never returned by the gateway.")
                }
            />
            <SecretRevealToggle
                caption="Show the API key while typing"
                isVisible={isKeyVisible()}
                onToggle={setIsKeyVisible}
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
