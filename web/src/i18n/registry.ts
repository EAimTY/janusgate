import english from "./catalogs/en";
import type { LocaleEntry } from "./types";

/** Production tags and loaders. Non-English resources stay outside the eager graph. */
export const localeRegistry = {
    en: {
        autonym: "English",
        load: async () => english,
    },
    "zh-CN": {
        autonym: "简体中文",
        load: async () => (await import("./catalogs/zh-CN")).default,
    },
} as const satisfies Record<string, LocaleEntry>;
export type SupportedLocale = keyof typeof localeRegistry;
export type LanguagePreference = "system" | SupportedLocale;
export const languageDefaults = {
    en: "en",
    zh: "zh-CN",
} as const satisfies Record<string, SupportedLocale>;

/** Matches candidates in browser preference order; registry ordering has no meaning. */
export function resolveBrowserLocale<T extends string>(
    languages: readonly string[],
    tags: readonly T[],
    defaults: Readonly<Record<string, T>>,
): T {
    const englishTag = tags.find((tag) => tag === "en");
    if (englishTag === undefined) throw new Error("Locale registry requires English");
    for (const candidate of languages) {
        let locale: Intl.Locale;
        try {
            locale = new Intl.Locale(candidate);
        } catch {
            continue;
        }
        const exact = tags.find((tag) => tag === locale.baseName);
        if (exact !== undefined) return exact;
        const script = locale.script ?? locale.maximize().script;
        const scriptTag = tags.find((tag) => tag === `${locale.language}-${script}`);
        if (scriptTag !== undefined) return scriptTag;
        const fallback = defaults[locale.language];
        if (fallback !== undefined) return fallback;
    }
    return englishTag;
}
