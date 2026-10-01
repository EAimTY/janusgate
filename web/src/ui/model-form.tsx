import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";
import { REASONING_LEVEL } from "/src/management-api/vocabulary";
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
 * The override a thinking-level write carries: the selected set while the
 * switch is on — the empty selection included, the positive declaration
 * that no level is supported — or `null` while it is off, so the discovered
 * set stands. The gateway canonicalizes the set on write (deduplicated,
 * vocabulary-ordered), so the selection's own order never matters.
 */
export function normalizedThinkingLevelsOverride(
    hasOverride: boolean,
    selected: ReadonlyArray<string>,
): Array<string> | null {
    return hasOverride
        ? [
              ...selected,
          ]
        : null;
}

/**
 * Whether two level sets declare the same levels, whatever order each was
 * picked in: the gateway canonicalizes on write, so a re-picked same set
 * in a different order is not a change worth saving.
 */
export function isSameLevelSet(
    before: ReadonlyArray<string> | null,
    after: ReadonlyArray<string> | null,
): boolean {
    if (before === null || after === null) {
        return before === after;
    }

    return before.length === after.length && before.every((level) => after.includes(level));
}

/**
 * The override a token-cap write carries: the parsed positive integer while
 * the switch is on and names one, or `null` — no override, so the
 * discovered value stands. An invalid entry reads as `null` here; the
 * inline error beside the field (not this function) is what holds the save.
 */
export function normalizedTokenCapOverride(hasOverride: boolean, text: string): number | null {
    if (!hasOverride) {
        return null;
    }

    const trimmed = text.trim();

    if (!/^\d+$/.test(trimmed)) {
        return null;
    }

    const value = Number(trimmed);

    return Number.isSafeInteger(value) && value > 0 ? value : null;
}

/**
 * The inline error for a token-cap override, shown where it was typed. A
 * switched-off override is always fine; a switched-on one must name a
 * positive integer — empty, fractional, zero, negative, or beyond the
 * safe-integer range is the same one rejection, naming the field.
 */
export function tokenCapOverrideError(
    hasOverride: boolean,
    text: string,
    label: string,
): string | null {
    if (!hasOverride) {
        return null;
    }

    return normalizedTokenCapOverride(true, text) === null
        ? `${label} must be a positive integer.`
        : null;
}

/**
 * A model's thinking-level override: the discovered set while the switch is
 * off, or a hand-picked set while it is on. An empty selection while on is
 * the positive declaration of no levels — not the same as off — and the
 * hint always names what the write produces, the way the public-ID
 * override's off-hint names the composed ID.
 */
export function ThinkingLevelsOverrideFields(props: {
    readonly hasOverride: boolean;
    readonly onHasOverrideToggle: (hasOverride: boolean) => void;
    readonly selected: ReadonlyArray<string>;
    readonly onSelectedChange: (selected: ReadonlyArray<string>) => void;
    readonly discovered: ReadonlyArray<string> | null;
    readonly isDisabled?: boolean;
}): JSX.Element {
    const discoveredLine = (): string => {
        if (props.discovered === null) {
            return "Following discovery — currently unknown, so listings omit it.";
        }

        return props.discovered.length === 0
            ? "Following discovery — currently declares no levels."
            : `Following discovery: ${props.discovered.join(", ")}.`;
    };

    const onHint = (): string =>
        props.selected.length === 0
            ? "Replaces the discovered set — an empty selection declares no levels."
            : `Replaces the discovered set — clients see ${props.selected.join(", ")}.`;

    const toggleLevel = (level: string, isChecked: boolean): void => {
        props.onSelectedChange(
            isChecked
                ? [
                      ...props.selected,
                      level,
                  ]
                : props.selected.filter((candidate) => candidate !== level),
        );
    };

    return (
        <div class="flex flex-col gap-4">
            <Switch
                label="Override reasoning levels"
                hint={props.hasOverride ? onHint() : discoveredLine()}
                isOn={props.hasOverride}
                isDisabled={props.isDisabled ?? false}
                onToggle={props.onHasOverrideToggle}
            />
            <Show when={props.hasOverride}>
                <fieldset disabled={props.isDisabled ?? false}>
                    <legend class="text-[13px] font-medium text-ink">Reasoning levels</legend>
                    <div class="mt-1.5 flex flex-wrap gap-2">
                        <For each={REASONING_LEVEL.options}>
                            {(option) => (
                                <label class="tnum inline-flex cursor-pointer items-center gap-1.5 rounded-lg border border-rule-strong bg-canvas px-2.5 py-1.5 text-[13px] text-ink transition-colors duration-150 ease-out has-checked:border-accent has-checked:bg-accent-soft has-checked:text-ink hover:border-ink-faint dark:bg-muted">
                                    <input
                                        type="checkbox"
                                        class="size-4 shrink-0 accent-accent"
                                        checked={props.selected.includes(option.value)}
                                        onChange={(event) =>
                                            toggleLevel(option.value, event.currentTarget.checked)
                                        }
                                    />
                                    {option.label}
                                </label>
                            )}
                        </For>
                    </div>
                    <p class="mt-1.5 max-w-[70ch] text-[13px] text-ink-faint">
                        Pick every level this model declares. Untick all of them to declare none.
                    </p>
                </fieldset>
            </Show>
        </div>
    );
}

/**
 * One token-cap override: the discovered value while the switch is off, or a
 * hand-typed positive integer while it is on. The hint always names what
 * the write produces.
 */
export function TokenCapOverrideFields(props: {
    readonly switchLabel: string;
    readonly fieldLabel: string;
    readonly hasOverride: boolean;
    readonly onHasOverrideToggle: (hasOverride: boolean) => void;
    readonly value: string;
    readonly onValueInput: (value: string) => void;
    readonly discovered: number | null;
    readonly valueError?: string | null | undefined;
    readonly isDisabled?: boolean;
}): JSX.Element {
    const discoveredLine = (): string =>
        props.discovered === null
            ? "Following discovery — currently unknown, so listings omit it."
            : `Following discovery: ${props.discovered.toLocaleString()}.`;

    const onHint = (): string => {
        const parsed = normalizedTokenCapOverride(true, props.value);

        return parsed === null
            ? "Replaces the discovered value with a positive integer."
            : `Replaces the discovered value — clients see ${parsed.toLocaleString()}.`;
    };

    return (
        <div class="flex flex-col gap-4">
            <Switch
                label={props.switchLabel}
                hint={props.hasOverride ? onHint() : discoveredLine()}
                isOn={props.hasOverride}
                isDisabled={props.isDisabled ?? false}
                onToggle={props.onHasOverrideToggle}
            />
            <Show when={props.hasOverride}>
                <TextField
                    label={props.fieldLabel}
                    value={props.value}
                    onInput={props.onValueInput}
                    isDisabled={props.isDisabled ?? false}
                    error={props.valueError}
                    hint="A positive integer. Clear the switch to follow discovery again."
                />
            </Show>
        </div>
    );
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
