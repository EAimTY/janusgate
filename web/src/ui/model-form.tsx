import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";
import { Switch, TextField } from "/src/ui/controls";

/**
 * The override a model write carries: the trimmed field while the switch is
 * on and names something, or `null` — no override, so the composed ID
 * stands. Both model forms derive their gates, writes, and hints from this
 * one function, so what the off-state hints is what the write sends.
 */
export function normalizedPublicIdOverride(
    hasIdOverride: boolean,
    idOverride: string,
): string | null {
    return !hasIdOverride || idOverride.trim() === "" ? null : idOverride.trim();
}

/**
 * The public ID a model serves under when no override stands: the owning
 * provider's prefix over the upstream ID, verbatim. A standing override
 * takes the whole place.
 */
export function composedPublicId(
    modelPrefix: string,
    upstreamId: string,
    override: string | null,
): string {
    return override ?? `${modelPrefix}${upstreamId}`;
}

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
