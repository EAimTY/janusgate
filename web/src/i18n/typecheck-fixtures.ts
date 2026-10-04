/** Compile-only proof of the facade's resource and interpolation contracts. */
import type { TFunction } from "i18next";
import { languageLoadingMessage } from "./language-messages";

export function checkTranslationTypes(t: TFunction<"translation">): void {
    t(($) => $.models.discoveredCount, {
        count: 2,
    });
    t(($) => $.language.loading, {
        languageName: "English",
    });
    // @ts-expect-error Unknown semantic selector keys are rejected.
    t(($) => $.common.missingMessage);
    // @ts-expect-error Interpolation parameters cannot be omitted.
    languageLoadingMessage(t, {});
    // @ts-expect-error The languageName interpolation parameter is required.
    t(($) => $.language.loading, {
        wrongName: "English",
    });
}
