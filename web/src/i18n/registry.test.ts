import { describe, expect, test } from "vitest";
import { languageDefaults, localeRegistry, resolveBrowserLocale } from "./registry";

/**
 * The locale registry's browser-language resolution: candidates walked in
 * browser preference order, matched exactly, then by the script a maximized
 * tag names, then by the language's default locale, with the registry's
 * English as the final fallback — and registry ordering carrying no meaning.
 */

// Object.keys loses the registry's key union; the tags are the registry's own.
const TAGS = Object.keys(localeRegistry) as Array<keyof typeof localeRegistry>;

describe("resolveBrowserLocale", () => {
    test("matches an exact registry tag", () => {
        expect(
            resolveBrowserLocale(
                [
                    "ja",
                ],
                TAGS,
                languageDefaults,
            ),
        ).toBe("ja");
        expect(
            resolveBrowserLocale(
                [
                    "zh-CN",
                ],
                TAGS,
                languageDefaults,
            ),
        ).toBe("zh-CN");
    });

    test("maximizes a tag to its script when only the script variant ships", () => {
        expect(
            resolveBrowserLocale(
                [
                    "zh-TW",
                ],
                TAGS,
                languageDefaults,
            ),
        ).toBe("zh-Hant");
        expect(
            resolveBrowserLocale(
                [
                    "zh-HK",
                ],
                TAGS,
                languageDefaults,
            ),
        ).toBe("zh-Hant");
    });

    test("falls back to the language's default locale", () => {
        expect(
            resolveBrowserLocale(
                [
                    "zh",
                ],
                TAGS,
                languageDefaults,
            ),
        ).toBe("zh-CN");
        expect(
            resolveBrowserLocale(
                [
                    "de",
                ],
                TAGS,
                languageDefaults,
            ),
        ).toBe("en");
    });

    test("walks candidates in browser preference order", () => {
        expect(
            resolveBrowserLocale(
                [
                    "de",
                    "ko",
                    "ja",
                ],
                TAGS,
                languageDefaults,
            ),
        ).toBe("ko");
        // A higher-preference candidate with only a script match beats a later exact one.
        expect(
            resolveBrowserLocale(
                [
                    "zh-TW",
                    "ja",
                ],
                TAGS,
                languageDefaults,
            ),
        ).toBe("zh-Hant");
    });

    test("skips candidates the locale grammar cannot parse", () => {
        expect(
            resolveBrowserLocale(
                [
                    "not a locale!",
                    "ja",
                ],
                TAGS,
                languageDefaults,
            ),
        ).toBe("ja");
    });

    test("answers English when nothing else matches, and for an empty list", () => {
        expect(
            resolveBrowserLocale(
                [
                    "fr",
                    "de",
                ],
                TAGS,
                languageDefaults,
            ),
        ).toBe("en");
        expect(resolveBrowserLocale([], TAGS, languageDefaults)).toBe("en");
    });

    test("carries the shipped registry: English pre-resolved, one loader per locale", () => {
        expect(TAGS.sort()).toEqual(
            [
                "en",
                "es",
                "ja",
                "ko",
                "zh-CN",
                "zh-Hant",
            ].sort(),
        );
        for (const tag of TAGS) {
            expect(localeRegistry[tag]?.autonym.length).toBeGreaterThan(0);
        }
    });
});
