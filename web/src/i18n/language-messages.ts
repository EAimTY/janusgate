import type { TFunction } from "i18next";

/** i18next permits an omitted options argument; this domain helper requires the name. */
export function languageLoadingMessage(
    t: TFunction<"translation">,
    parameters: {
        readonly languageName: string;
    },
): string {
    return t(($) => $.language.loading, parameters);
}
