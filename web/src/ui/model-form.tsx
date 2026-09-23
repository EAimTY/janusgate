import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";
import { Switch, TextField } from "/src/ui/controls";

/**
 * A model's public ID override: the composed ID while the switch is off, or
 * a hand-set name while it is on. An empty value while on is the same as
 * off — the composed ID stands.
 */
export function ModelIdOverrideFields(props: {
    readonly hasIdOverride: boolean;
    readonly onHasIdOverrideToggle: (hasIdOverride: boolean) => void;
    readonly idOverride: string;
    readonly onIdOverrideInput: (value: string) => void;
    readonly composedId: string;
    readonly idOverrideError?: string | null | undefined;
    readonly isDisabled?: boolean;
}): JSX.Element {
    const offHint = (): string =>
        props.composedId === ""
            ? "Clients call the composed ID once the upstream ID is named."
            : `Clients call ${props.composedId}.`;

    return (
        <div class="flex flex-col gap-4">
            <Switch
                label="Override the public ID"
                hint={props.hasIdOverride ? "Clients call the override below." : offHint()}
                isOn={props.hasIdOverride}
                isDisabled={props.isDisabled ?? false}
                onToggle={props.onHasIdOverrideToggle}
            />
            <Show when={props.hasIdOverride}>
                <TextField
                    label="Public ID override"
                    value={props.idOverride}
                    onInput={props.onIdOverrideInput}
                    isDisabled={props.isDisabled ?? false}
                    error={props.idOverrideError}
                    hint="Replaces the composed ID. Renaming breaks clients on the old ID."
                />
            </Show>
        </div>
    );
}
