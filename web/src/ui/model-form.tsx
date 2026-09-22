import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";
import { Switch, TextField } from "/src/ui/controls";

/**
 * A model's public ID override: the composed ID while the switch is off, or
 * a hand-set name while it is on. An empty value while on is the same as
 * off — the composed ID stands.
 */
export function ModelAliasFields(props: {
    readonly hasAlias: boolean;
    readonly onHasAliasToggle: (hasAlias: boolean) => void;
    readonly alias: string;
    readonly onAliasInput: (value: string) => void;
    readonly composedId: string;
    readonly isDisabled?: boolean;
}): JSX.Element {
    const offHint = (): string =>
        props.composedId === ""
            ? "Off: clients call the composed ID once the upstream ID is named."
            : `Off: clients call ${props.composedId}.`;

    return (
        <div class="flex flex-col gap-4">
            <Switch
                label="Override the public ID"
                hint={props.hasAlias ? "On: clients call the override below." : offHint()}
                isOn={props.hasAlias}
                onText="On"
                offText="Off"
                isDisabled={props.isDisabled ?? false}
                onToggle={props.onHasAliasToggle}
            />
            <Show when={props.hasAlias}>
                <TextField
                    label="Public ID override"
                    value={props.alias}
                    onInput={props.onAliasInput}
                    isDisabled={props.isDisabled ?? false}
                    hint="Replaces the composed ID. Renaming breaks clients on the old ID."
                />
            </Show>
        </div>
    );
}
