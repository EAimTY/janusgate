import type { JSX } from "@solidjs/web";
import { For } from "solid-js";
import type {
    OAuthProviderType,
    ProviderResponse,
    UpstreamFormat,
} from "/src/management-api/generated/v0";
import {
    OAUTH_PROVIDER_TYPE_OPTIONS,
    providerTypeLabel,
    UPSTREAM_FORMAT_OPTIONS,
} from "/src/management-api/vocabulary";
import { ariaBoolean, joinClass } from "/src/ui/attributes";
import { SelectField, Switch, TextField } from "/src/ui/controls";

/** The account kinds the creation form offers, in one vocabulary. */
export type ProviderKind = ProviderResponse["type"];

export const PROVIDER_KIND_OPTIONS: ReadonlyArray<{
    readonly value: ProviderKind;
    readonly title: string;
    readonly description: string;
}> = [
    {
        value: "openai_compatible",
        title: "API key",
        description: "Any OpenAI-compatible service, authenticated with a static key.",
    },
    ...OAUTH_PROVIDER_TYPE_OPTIONS.map((option) => ({
        value: option.value as OAuthProviderType,
        title: option.value === "openai_oauth" ? "ChatGPT account" : "Google account",
        description:
            option.value === "openai_oauth"
                ? "An OpenAI account, connected through its login page."
                : "A Google account, connected through its login page.",
    })),
];

/**
 * The provider kind as three plain options. OAuth kinds and the static key
 * are one choice here — not two different creations — because they are all
 * providers whose credential paths merely differ.
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

/** Address, wire format, and key of an API-key provider. */
export function CompatibleConnectionFields(props: {
    readonly baseUrl: string;
    readonly protocol: UpstreamFormat;
    readonly apiKey: string;
    readonly onBaseUrlInput: (value: string) => void;
    readonly onProtocolChange: (value: UpstreamFormat) => void;
    readonly onApiKeyInput: (value: string) => void;
    readonly apiKeyLabel?: string | undefined;
    readonly apiKeyHint?: string | undefined;
    readonly isDisabled?: boolean;
}): JSX.Element {
    return (
        <div class="flex flex-col gap-4">
            <div class="grid gap-4 sm:grid-cols-2">
                <TextField
                    label="Upstream base URL"
                    type="url"
                    value={props.baseUrl}
                    onInput={props.onBaseUrlInput}
                    placeholder="https://upstream.example.com/v1"
                    isDisabled={props.isDisabled ?? false}
                    hint="Where this provider's traffic is sent."
                />
                <SelectField
                    label="Upstream protocol"
                    value={props.protocol}
                    options={UPSTREAM_FORMAT_OPTIONS}
                    onChange={props.onProtocolChange}
                    isDisabled={props.isDisabled ?? false}
                    hint="The wire format this upstream speaks."
                />
            </div>
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

/** The label a stored provider row's own type is shown under. */
export function providerKindTitle(type: ProviderKind): string {
    return type === "openai_compatible" ? "API key" : providerTypeLabel(type);
}
