import { describe, expect, it } from "vitest";
import { resolveBrowserLocale } from "./registry";

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
});
