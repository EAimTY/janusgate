import { describe, expect, it } from "vitest";
import { languageDefaults, localeRegistry, resolveBrowserLocale } from "./registry";

describe("browser locale intent", () => {
    it("uses browser order, canonical bases, and explicit language defaults", () => {
        const tags = [
            "zh-CN",
            "en",
        ];
        const defaults = {
            en: "en",
            zh: "zh-CN",
        };
        for (const [languages, expected] of [
            [
                [
                    "en-US",
                    "zh-CN",
                ],
                "en",
            ],
            [
                [
                    "fr-FR",
                    "zh-SG",
                    "en",
                ],
                "zh-CN",
            ],
            [
                [
                    "zh-TW",
                    "en",
                ],
                "zh-CN",
            ],
            [
                [
                    "ar",
                    "fr",
                ],
                "en",
            ],
            [
                [
                    "invalid_!",
                    "ZH-cn-u-nu-hanidec",
                ],
                "zh-CN",
            ],
        ] as const) {
            expect(resolveBrowserLocale(languages, tags, defaults)).toBe(expected);
        }
    });
    it("prefers an inferred script over the generic default", () => {
        const tags = [
            "en",
            "zh-CN",
            "zh-Hans",
            "zh-Hant",
        ];
        const defaults = {
            en: "en",
            zh: "zh-CN",
        };
        expect(
            resolveBrowserLocale(
                [
                    "zh-TW",
                ],
                tags,
                defaults,
            ),
        ).toBe("zh-Hant");
        expect(
            resolveBrowserLocale(
                [
                    "zh-SG",
                ],
                tags,
                defaults,
            ),
        ).toBe("zh-Hans");
        expect(
            resolveBrowserLocale(
                [
                    "zh-Hant-x-private",
                ],
                tags,
                defaults,
            ),
        ).toBe("zh-Hant");
    });
    it("resolves the shipped registry across regions and scripts", () => {
        const tags = Object.keys(localeRegistry) as Array<keyof typeof localeRegistry>;
        for (const [languages, expected] of [
            [
                [
                    "zh-TW",
                ],
                "zh-Hant",
            ],
            [
                [
                    "zh-HK",
                ],
                "zh-Hant",
            ],
            [
                [
                    "zh-SG",
                ],
                "zh-CN",
            ],
            [
                [
                    "zh",
                ],
                "zh-CN",
            ],
            [
                [
                    "ja-JP",
                ],
                "ja",
            ],
            [
                [
                    "ko-KR",
                ],
                "ko",
            ],
            [
                [
                    "es-419",
                ],
                "es",
            ],
            [
                [
                    "de-DE",
                ],
                "en",
            ],
        ] as const)
            expect(resolveBrowserLocale(languages, tags, languageDefaults)).toBe(expected);
    });
});
