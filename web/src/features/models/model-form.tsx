import type { JSX } from "@solidjs/web";
import { type Accessor, createSignal, For, Show } from "solid-js";
import { translation } from "/src/i18n";
import { formatNumber } from "/src/i18n/formatters";
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
function normalizedThinkingLevelsOverride(
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
function normalizedTokenCapOverride(hasOverride: boolean, text: string): number | null {
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
function tokenCapOverrideError(
    hasOverride: boolean,
    text: string,
    field: "maxInputOverride" | "maxOutputOverride",
): string | null {
    if (!hasOverride) {
        return null;
    }

    return normalizedTokenCapOverride(true, text) === null
        ? translation()(($) => $.validation.positive[field])
        : null;
}

/**
 * The capability-override draft the model forms edit: the three override
 * groups' signals — thinking levels, the two token caps, and the compaction
 * facts — with the inline errors and the normalized wire values derived from
 * them, and `apply` re-seeding the whole bundle from a row (a save's
 * returned row, or the baseline a revert picks up). A `null` initial seeds
 * the creation shape: every override off, every field clear.
 */
type CapabilityOverrideDraft = {
    readonly hasLevelsOverride: Accessor<boolean>;
    readonly setHasLevelsOverride: (hasOverride: boolean) => void;
    readonly selectedLevels: Accessor<ReadonlyArray<string>>;
    readonly setSelectedLevels: (selected: ReadonlyArray<string>) => void;
    readonly hasMaxInputOverride: Accessor<boolean>;
    readonly setHasMaxInputOverride: (hasOverride: boolean) => void;
    readonly maxInputText: Accessor<string>;
    readonly setMaxInputText: (text: string) => void;
    readonly hasMaxOutputOverride: Accessor<boolean>;
    readonly setHasMaxOutputOverride: (hasOverride: boolean) => void;
    readonly maxOutputText: Accessor<string>;
    readonly setMaxOutputText: (text: string) => void;
    readonly hasCompactionOverride: Accessor<boolean>;
    readonly setHasCompactionOverride: (hasOverride: boolean) => void;
    readonly selectedCompaction: Accessor<CompactionOverrideSelection>;
    readonly setSelectedCompaction: (selected: CompactionOverrideSelection) => void;
    /** The inline error beside each token-cap field, `null` while its entry is valid. */
    readonly maxInputError: Accessor<string | null>;
    readonly maxOutputError: Accessor<string | null>;
    /** The override each group's write carries — `null` where the switch is off. */
    readonly normalizedLevels: Accessor<Array<string> | null>;
    readonly normalizedMaxInput: Accessor<number | null>;
    readonly normalizedMaxOutput: Accessor<number | null>;
    readonly normalizedCompaction: Accessor<ModelResponse["compaction_support_override"]>;
    /** Re-seeds every field from one row's stored overrides. */
    readonly apply: (model: ModelResponse) => void;
};

/** The capability-override draft over one model's fields: `null` seeds a creation's empty shape. */
export function createCapabilityOverrideDraft(
    initial: ModelResponse | null,
): CapabilityOverrideDraft {
    const [hasLevelsOverride, setHasLevelsOverride] = createSignal(
        initial !== null && initial.thinking_levels_override !== null,
    );
    const [selectedLevels, setSelectedLevels] = createSignal<ReadonlyArray<string>>(
        initial?.thinking_levels_override ?? [],
    );
    const [hasMaxInputOverride, setHasMaxInputOverride] = createSignal(
        initial !== null && initial.max_input_tokens_override !== null,
    );
    const [maxInputText, setMaxInputText] = createSignal(
        initial?.max_input_tokens_override?.toString() ?? "",
    );
    const [hasMaxOutputOverride, setHasMaxOutputOverride] = createSignal(
        initial !== null && initial.max_output_tokens_override !== null,
    );
    const [maxOutputText, setMaxOutputText] = createSignal(
        initial?.max_output_tokens_override?.toString() ?? "",
    );
    const [hasCompactionOverride, setHasCompactionOverride] = createSignal(
        initial !== null && initial.compaction_support_override !== null,
    );
    const [selectedCompaction, setSelectedCompaction] = createSignal(
        compactionOverrideSelection(initial?.compaction_support_override ?? null),
    );

    return {
        hasLevelsOverride,
        setHasLevelsOverride,
        selectedLevels,
        setSelectedLevels,
        hasMaxInputOverride,
        setHasMaxInputOverride,
        maxInputText,
        setMaxInputText,
        hasMaxOutputOverride,
        setHasMaxOutputOverride,
        maxOutputText,
        setMaxOutputText,
        hasCompactionOverride,
        setHasCompactionOverride,
        selectedCompaction,
        setSelectedCompaction,
        maxInputError: () =>
            tokenCapOverrideError(hasMaxInputOverride(), maxInputText(), "maxInputOverride"),
        maxOutputError: () =>
            tokenCapOverrideError(hasMaxOutputOverride(), maxOutputText(), "maxOutputOverride"),
        normalizedLevels: () =>
            normalizedThinkingLevelsOverride(hasLevelsOverride(), selectedLevels()),
        normalizedMaxInput: () => normalizedTokenCapOverride(hasMaxInputOverride(), maxInputText()),
        normalizedMaxOutput: () =>
            normalizedTokenCapOverride(hasMaxOutputOverride(), maxOutputText()),
        normalizedCompaction: () =>
            normalizedCompactionOverride(hasCompactionOverride(), selectedCompaction()),
        apply: (model) => {
            setHasLevelsOverride(model.thinking_levels_override !== null);
            setSelectedLevels(model.thinking_levels_override ?? []);
            setHasMaxInputOverride(model.max_input_tokens_override !== null);
            setMaxInputText(model.max_input_tokens_override?.toString() ?? "");
            setHasMaxOutputOverride(model.max_output_tokens_override !== null);
            setMaxOutputText(model.max_output_tokens_override?.toString() ?? "");
            setHasCompactionOverride(model.compaction_support_override !== null);
            setSelectedCompaction(compactionOverrideSelection(model.compaction_support_override));
        },
    };
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
            return translation()(
                ($) => $.helpers.modelForm.followingDiscoveryCurrentlyUnknownSoListingsOmitIt,
            );
        }

        return props.discovered.length === 0
            ? translation()(($) => $.helpers.modelForm.followingDiscoveryCurrentlyDeclaresNoLevels)
            : translation()(($) => $.dynamicMessages.following, {
                  value: props.discovered
                      .map((level) =>
                          REASONING_LEVEL.label(
                              REASONING_LEVEL.parse(level, "discovered reasoning level"),
                          ),
                      )
                      .join(", "),
              });
    };

    const onHint = (): string =>
        props.selected.length === 0
            ? translation()(
                  ($) =>
                      $.helpers.modelForm.replacesTheDiscoveredSetAnEmptySelectionDeclaresNoLevels,
              )
            : translation()(($) => $.dynamicMessages.levelsReplace, {
                  value: props.selected
                      .map((level) =>
                          REASONING_LEVEL.label(
                              REASONING_LEVEL.parse(level, "selected reasoning level"),
                          ),
                      )
                      .join(", "),
              });

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
                label={translation()(($) => $.controls.overrideReasoningLevels)}
                hint={props.hasOverride ? onHint() : discoveredLine()}
                isOn={props.hasOverride}
                isDisabled={props.isDisabled ?? false}
                onToggle={props.onHasOverrideToggle}
            />
            <Show when={props.hasOverride}>
                <fieldset disabled={props.isDisabled ?? false}>
                    <legend class="text-[13px] font-medium text-ink">
                        {translation()(($) => $.controls.reasoningLevels)}
                    </legend>
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
                        {translation()(($) => $.controls.pickEveryLevelThisModelDeclaresUntickAll)}
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
            ? translation()(
                  ($) => $.helpers.modelForm.followingDiscoveryCurrentlyUnknownSoListingsOmitIt,
              )
            : translation()(($) => $.dynamicMessages.following, {
                  value: formatNumber(props.discovered),
              });

    const onHint = (): string => {
        const parsed = normalizedTokenCapOverride(true, props.value);

        return parsed === null
            ? translation()(
                  ($) => $.helpers.modelForm.replacesTheDiscoveredValueWithAPositiveInteger,
              )
            : translation()(($) => $.dynamicMessages.capReplace, {
                  value: formatNumber(parsed),
              });
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
                    hint={translation()(($) => $.controls.aPositiveIntegerClearTheSwitchToFollow)}
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
type CompactionOverrideChoice = "unset" | MechanismSupport;

/** The per-mechanism override choices of one compaction-facts edit. */
type CompactionOverrideSelection = Readonly<Record<CompactionMechanism, CompactionOverrideChoice>>;

/**
 * The editor's selection a stored override map opens with: a stated member
 * carries its state, an unstated one sits at `"unset"`. The record states
 * every mechanism explicitly, so a mechanism added to the vocabulary fails
 * to compile until the selection carries it.
 */
function compactionOverrideSelection(
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
function normalizedCompactionOverride(
    hasOverride: boolean,
    selected: CompactionOverrideSelection,
): ModelResponse["compaction_support_override"] {
    if (!hasOverride) {
        return null;
    }

    const override: {
        [mechanism: string]: MechanismSupport;
    } = {};

    for (const value of COMPACTION_MECHANISM.values) {
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
            return translation()(
                ($) => $.helpers.modelForm.followingDiscoveryCurrentlyNothingIsKnownSoTheMessage,
            );
        }

        const stated = COMPACTION_MECHANISM.options.flatMap(({ value }) => {
            const support = discovered[value];

            return support === undefined
                ? []
                : [
                      translation()(($) => $.dynamicMessages.mechanismFact, {
                          mechanism: COMPACTION_MECHANISM.label(value),
                          support: MECHANISM_SUPPORT.label(support),
                      }),
                  ];
        });

        if (stated.length === 0) {
            return translation()(
                ($) => $.helpers.modelForm.followingDiscoveryCurrentlyNothingIsKnownSoTheMessage,
            );
        }

        return translation()(($) => $.dynamicMessages.following, {
            value: stated.join("; "),
        });
    };

    const onHint = (): string => translation()(($) => $.dynamicMessages.compactionReplace);

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
            get label() {
                return translation()(($) => $.helpers.modelForm.unset);
            },
        },
        {
            value: "supported",
            get label() {
                return MECHANISM_SUPPORT.label("supported");
            },
        },
        {
            value: "unsupported",
            get label() {
                return MECHANISM_SUPPORT.label("unsupported");
            },
        },
        {
            value: "unknown",
            get label() {
                return MECHANISM_SUPPORT.label("unknown");
            },
        },
    ];

    const discoveredLine = (mechanism: CompactionMechanism): string => {
        const support = props.discovered?.[mechanism];

        return support === undefined
            ? translation()(($) => $.helpers.modelForm.discoveredNothingKnown)
            : translation()(($) => $.dynamicMessages.mechanismDiscovered, {
                  support: MECHANISM_SUPPORT.label(support),
              });
    };

    return (
        <div class="flex flex-col gap-4">
            <Switch
                label={translation()(($) => $.controls.overrideCompactionFacts)}
                hint={props.hasOverride ? onHint() : offHint()}
                isOn={props.hasOverride}
                isDisabled={props.isDisabled ?? false}
                onToggle={props.onHasOverrideToggle}
            />
            <Show when={props.hasOverride}>
                <fieldset disabled={props.isDisabled ?? false}>
                    <legend class="text-[13px] font-medium text-ink">
                        {translation()(($) => $.controls.compactionMechanisms)}
                    </legend>
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
                        {translation()(
                            ($) => $.controls.stateEveryMechanismWhoseFactsThisModelReplaces,
                        )}
                    </p>
                </fieldset>
            </Show>
        </div>
    );
}
