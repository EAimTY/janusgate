import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";
import { Button, SelectField } from "/src/ui/controls";
import {
    languageChangeState,
    languagePreference,
    localeRegistry,
    retryLanguageChange,
    setLanguagePreference,
    translation,
} from "./index";
import { languageLoadingMessage } from "./language-messages";
import type { LanguagePreference } from "./registry";

/** Preference and load feedback stay outside gateway notice ownership. */
export function LanguageControl(): JSX.Element {
    const preference = (): LanguagePreference => {
        const state = languageChangeState();
        return state.type === "loading" ? state.preference : languagePreference();
    };
    const options = (): ReadonlyArray<{
        value: LanguagePreference;
        label: string;
    }> => [
        {
            value: "system",
            label: translation()(($) => $.language.system),
        },
        ...Object.entries(localeRegistry).map(([tag, entry]) => ({
            value: tag as keyof typeof localeRegistry,
            label: entry.autonym,
        })),
    ];
    const status = (): string => {
        const state = languageChangeState();
        if (state.type === "idle") return "";
        const languageName = localeRegistry[state.locale].autonym;
        if (state.type === "loading")
            return languageLoadingMessage(translation(), {
                languageName,
            });
        return state.isStartupRecovery
            ? translation()(($) => $.language.startupFailed, {
                  languageName,
              })
            : translation()(($) => $.language.failed, {
                  languageName,
              });
    };
    return (
        <div class="min-w-0">
            <SelectField
                label={translation()(($) => $.common.language)}
                value={preference()}
                options={options()}
                onChange={(next) => {
                    void setLanguagePreference(next);
                }}
            />
            <div
                aria-live="polite"
                class="max-w-[36ch] text-[13px] text-ink-dim"
            >
                <Show when={status()}>{(message) => <p class="mt-1.5">{message()}</p>}</Show>
                <Show when={languageChangeState().type === "failed"}>
                    <Button
                        onClick={() => {
                            void retryLanguageChange();
                        }}
                        class="mt-1.5"
                    >
                        {translation()(($) => $.common.retry)}
                    </Button>
                </Show>
            </div>
        </div>
    );
}
