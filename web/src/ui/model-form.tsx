import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";
import type { MechanismSupport, ModelResponse } from "/src/management-api/generated/v0";
import {
    COMPACTION_MECHANISM,
    type CompactionMechanism,
    MECHANISM_SUPPORT,
    REASONING_LEVEL,
} from "/src/management-api/vocabulary";
import { SelectField, Switch, TextField } from "/src/ui/controls";

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
 * One mechanism's override choice in the compaction-facts editor: `"unset"`
 * leaves the mechanism out of the override map, so the discovered fact and
 * then the definition's declaration decide; the three support states are
 * the explicit members the wire carries.
 */
export type CompactionOverrideChoice = "unset" | MechanismSupport;

/** The per-mechanism override choices of one compaction-facts edit. */
export type CompactionOverrideSelection = Readonly<
    Record<CompactionMechanism, CompactionOverrideChoice>
>;

/**
 * The editor's selection a stored override map opens with: a stated member
 * carries its state, an unstated one sits at `"unset"`. The record states
 * every mechanism explicitly, so a mechanism added to the vocabulary fails
 * to compile until the selection carries it.
 */
export function compactionOverrideSelection(
    override: ModelResponse["compaction_support_override"],
): CompactionOverrideSelection {
    return {
        responses_standalone_compact: override?.["responses_standalone_compact"] ?? "unset",
        responses_explicit_trigger: override?.["responses_explicit_trigger"] ?? "unset",
        responses_automatic_compaction: override?.["responses_automatic_compaction"] ?? "unset",
        messages_on_demand_compaction: override?.["messages_on_demand_compaction"] ?? "unset",
        messages_threshold_compaction: override?.["messages_threshold_compaction"] ?? "unset",
    };
}

/**
 * The override a compaction-facts write carries: the stated mechanisms'
 * map while the switch is on — the mechanism spellings are the closed
 * keys, and a mechanism left `"unset"` is absent from the map, letting the
 * discovered fact and the definition's declaration decide — or `null`
 * while it is off, so the discovered facts stand.
 */
export function normalizedCompactionOverride(
    hasOverride: boolean,
    selected: CompactionOverrideSelection,
): ModelResponse["compaction_support_override"] {
    if (!hasOverride) {
        return null;
    }

    const override: {
        [mechanism: string]: MechanismSupport;
    } = {};

    for (const { value } of COMPACTION_MECHANISM.options) {
        const choice = selected[value];

        if (choice !== "unset") {
            override[value] = choice;
        }
    }

    return override;
}

/** Whether two override maps state the same mechanisms at the same states. */
export function isSameCompactionOverride(
    before: ModelResponse["compaction_support_override"],
    after: ModelResponse["compaction_support_override"],
): boolean {
    if (before === null || after === null) {
        return before === after;
    }

    const beforeMechanisms = Object.keys(before);

    return (
        beforeMechanisms.length === Object.keys(after).length &&
        beforeMechanisms.every((mechanism) => after[mechanism] === before[mechanism])
    );
}

/**
 * A model's compaction-facts override: the discovered facts while the
 * switch is off, or a per-mechanism statement while it is on — each
 * mechanism set to a support state or left unset. The override replaces
 * the discovered facts wholesale, and an explicit `"unknown"` states "no
 * fact" loudly, suppressing the discovered fact for that mechanism.
 */
export function CompactionSupportOverrideFields(props: {
    readonly hasOverride: boolean;
    readonly onHasOverrideToggle: (hasOverride: boolean) => void;
    readonly selected: CompactionOverrideSelection;
    readonly onSelectedChange: (selected: CompactionOverrideSelection) => void;
    readonly discovered: ModelResponse["compaction_support"];
    readonly isDisabled?: boolean;
}): JSX.Element {
    const offHint = (): string => {
        const discovered = props.discovered;

        if (discovered === null) {
            return "Following discovery — currently nothing is known, so the definition's declaration decides.";
        }

        const stated = COMPACTION_MECHANISM.options.flatMap(({ value }) => {
            const support = discovered[value];

            return support === undefined
                ? []
                : [
                      `${COMPACTION_MECHANISM.label(value)} ${MECHANISM_SUPPORT.label(support).toLowerCase()}`,
                  ];
        });

        if (stated.length === 0) {
            return "Following discovery — currently nothing is known, so the definition's declaration decides.";
        }

        return `Following discovery: ${stated.join("; ")}.`;
    };

    const onHint = (): string =>
        "Replaces the discovered facts wholesale — a mechanism left unset falls back to discovery, then the definition's declaration.";

    const choose = (mechanism: CompactionMechanism, choice: CompactionOverrideChoice): void => {
        const next: Record<CompactionMechanism, CompactionOverrideChoice> = {
            ...props.selected,
        };
        next[mechanism] = choice;
        props.onSelectedChange(next);
    };

    const choiceOptions: ReadonlyArray<{
        readonly value: CompactionOverrideChoice;
        readonly label: string;
    }> = [
        {
            value: "unset",
            label: "Unset",
        },
        {
            value: "supported",
            label: MECHANISM_SUPPORT.label("supported"),
        },
        {
            value: "unsupported",
            label: MECHANISM_SUPPORT.label("unsupported"),
        },
        {
            value: "unknown",
            label: MECHANISM_SUPPORT.label("unknown"),
        },
    ];

    const discoveredLine = (mechanism: CompactionMechanism): string => {
        const support = props.discovered?.[mechanism];

        return support === undefined
            ? "Discovered: nothing known."
            : `Discovered: ${MECHANISM_SUPPORT.label(support)}.`;
    };

    return (
        <div class="flex flex-col gap-4">
            <Switch
                label="Override compaction facts"
                hint={props.hasOverride ? onHint() : offHint()}
                isOn={props.hasOverride}
                isDisabled={props.isDisabled ?? false}
                onToggle={props.onHasOverrideToggle}
            />
            <Show when={props.hasOverride}>
                <fieldset disabled={props.isDisabled ?? false}>
                    <legend class="text-[13px] font-medium text-ink">Compaction mechanisms</legend>
                    <div class="mt-1.5 flex max-w-xl flex-col gap-3">
                        <For each={COMPACTION_MECHANISM.options}>
                            {(option) => (
                                <SelectField
                                    label={option.label}
                                    value={props.selected[option.value]}
                                    options={choiceOptions}
                                    onChange={(choice) => choose(option.value, choice)}
                                    hint={discoveredLine(option.value)}
                                    isDisabled={props.isDisabled ?? false}
                                />
                            )}
                        </For>
                    </div>
                    <p class="mt-1.5 max-w-[70ch] text-[13px] text-ink-faint">
                        State every mechanism whose facts this model replaces. Unknown states "no
                        fact" loudly — it suppresses the discovered fact for that mechanism.
                    </p>
                </fieldset>
            </Show>
        </div>
    );
}
