import { describe, expect, it } from "vitest";
import { checkCatalog } from "./catalog-check";
import english from "./catalogs/en";
import { createLocalizationController } from "./controller";
import arabic from "./fixtures/ar";

describe("future Arabic catalog integration", () => {
    it("dispatches six cardinal branches through the application's fixed translator", async () => {
        const controller = createLocalizationController({
            registry: {
                en: {
                    autonym: "English",
                    load: async () => english,
                },
                ar: {
                    autonym: "العربية",
                    load: async () => arabic,
                },
            },
            defaults: {
                en: "en",
                ar: "ar",
            },
            readPreference: () => "ar",
            writePreference: () => {},
            browserLanguages: () => [
                "ar",
            ],
            eventTarget: window,
            root: document.documentElement,
            deadlineMs: 15_000,
        });
        await controller.initialize();
        expect(document.documentElement.dir).toBe("rtl");
        expect(controller.activeLocale()).toBe("ar");
        for (const [count, expected] of [
            [
                0,
                "لم يُكتشف أي نموذج (0).",
            ],
            [
                1,
                "اكتُشف نموذج واحد (1).",
            ],
            [
                2,
                "اكتُشف نموذجان (2).",
            ],
            [
                3,
                "اكتُشفت 3 نماذج.",
            ],
            [
                11,
                "اكتُشف 11 نموذجًا.",
            ],
            [
                100,
                "اكتُشف 100 نموذج.",
            ],
        ] as const)
            expect(
                controller.translation()(($) => $.models.discoveredCount, {
                    count,
                }),
            ).toBe(expected);
        expect(
            checkCatalog(
                {
                    models: english.models,
                },
                {
                    models: arabic.models,
                },
                "ar",
                [
                    {
                        path: "models.discoveredCount",
                        kind: "cardinal",
                    },
                ],
                [],
            ),
        ).toEqual([]);
        controller.dispose();
    });
});
