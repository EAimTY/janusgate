import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";
import { describeLevels } from "/src/format";
import { THINKING_LEVEL_ORDER, type ThinkingLevel } from "/src/management-api/generated/v0";
import { ariaBoolean, joinClass } from "/src/ui/attributes";

/**
 * The allowed thinking levels of one model: the canonical levels in
 * increasing strength, each either allowed or not. The stored override is a
 * set — the gateway accepts no empty set — so clearing the last level returns
 * the row to passthrough instead.
 */
export function ThinkingLevels(props: {
    readonly allowedLevels: ReadonlyArray<ThinkingLevel> | null | undefined;
    readonly onChange: (levels: ReadonlyArray<ThinkingLevel> | null) => void;
    readonly isDisabled?: boolean;
    readonly class?: string | undefined;
}): JSX.Element {
    const allowed = (): ReadonlyArray<ThinkingLevel> | null => props.allowedLevels ?? null;

    const isAllowed = (level: ThinkingLevel): boolean => allowed()?.includes(level) ?? false;

    const toggle = (level: ThinkingLevel): void => {
        const current = allowed();

        if (current === null) {
            props.onChange([
                level,
            ]);
            return;
        }

        const next = isAllowed(level)
            ? current.filter((candidate) => candidate !== level)
            : [
                  ...current,
                  level,
              ];

        props.onChange(next.length === 0 ? null : next);
    };

    return (
        <fieldset class={props.class}>
            <div class="flex items-baseline justify-between gap-3">
                <legend class="text-[13px] font-medium text-ink">Thinking levels</legend>
                <span class="text-[12.5px] font-medium text-ink-dim">
                    {allowed() === null ? "Passthrough" : "Limited"}
                </span>
            </div>
            <div class="mt-2 flex items-stretch gap-1.5">
                <For each={THINKING_LEVEL_ORDER}>
                    {(level) => (
                        <button
                            type="button"
                            aria-pressed={ariaBoolean(isAllowed(level))}
                            disabled={props.isDisabled ?? false}
                            onClick={() => toggle(level)}
                            class={joinClass(
                                "flex-1 cursor-pointer rounded-lg border px-2 py-2 text-[12.5px] font-medium transition-colors duration-150 ease-out disabled:cursor-not-allowed disabled:opacity-60",
                                isAllowed(level)
                                    ? "border-accent bg-accent-soft text-accent dark:text-accent-strong"
                                    : "border-rule-strong bg-surface text-ink-faint hover:border-ink-faint hover:text-ink-dim",
                            )}
                        >
                            {level}
                        </button>
                    )}
                </For>
            </div>
            <p class="mt-2 max-w-[70ch] text-[13px] text-ink-faint">
                <Show
                    when={allowed() === null}
                    fallback={`Currently allowing ${describeLevels(allowed())}. A request above the top allowed level is answered at it; below the lowest, at the lowest; with no level, at the top allowed level.`}
                >
                    The client's own level passes through untouched.
                </Show>
            </p>
        </fieldset>
    );
}
