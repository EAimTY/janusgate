import { describe, expect, it } from "vitest";
import { checkCatalog, checkRegistry } from "./catalog-check";
import english from "./catalogs/en";
import spanish from "./catalogs/es";
import japanese from "./catalogs/ja";
import korean from "./catalogs/ko";
import chinese from "./catalogs/zh-CN";
import traditionalChinese from "./catalogs/zh-Hant";
import { languageDefaults, localeRegistry } from "./registry";

/** Every shipped catalog, keyed by its registry tag. */
const shippedCatalogs = [
    {
        tag: "en",
        catalog: english,
    },
    {
        tag: "zh-CN",
        catalog: chinese,
    },
    {
        tag: "zh-Hant",
        catalog: traditionalChinese,
    },
    {
        tag: "ja",
        catalog: japanese,
    },
    {
        tag: "ko",
        catalog: korean,
    },
    {
        tag: "es",
        catalog: spanish,
    },
] as const;

describe("production catalog contracts", () => {
    it("validates all shipped resources and registry metadata", async () => {
        for (const { tag, catalog } of shippedCatalogs)
            expect(checkCatalog(english, catalog, tag), tag).toEqual([]);
        expect(await checkRegistry(localeRegistry, languageDefaults)).toEqual([]);
        for (const { tag, catalog } of shippedCatalogs)
            expect(await localeRegistry[tag].load(), tag).toBe(catalog);
    });
    it.each([
        {
            common: {},
        },
        {
            ...chinese,
            extra: "extra",
        },
        {
            ...chinese,
            common: "wrong shape",
        },
        {
            ...chinese,
            language: {
                ...chinese.language,
                loading: "{{wrong}}",
            },
        },
        {
            ...chinese,
            language: {
                ...chinese.language,
                loading: "{{ languageName",
            },
        },
        {
            ...chinese,
            models: {
                discoveredCount_one: "{{count}}",
            },
        },
        {
            ...chinese,
            common: {
                ...chinese.common,
                invented_one: "invalid",
            },
        },
    ])("rejects a broken semantic inventory or message", (catalog) => {
        expect(checkCatalog(english, catalog, "zh-CN").length).toBeGreaterThan(0);
    });
    it("requires string values in rich messages to use named slots", () => {
        const reference = {
            sentence: "[[name]] {{unsafe}}",
        };
        expect(
            checkCatalog(
                reference,
                reference,
                "en",
                [],
                [
                    {
                        path: "sentence",
                        slots: [
                            "name",
                        ],
                    },
                ],
            ).join(" "),
        ).toContain("string values require rich slots");
    });
    it("accepts language-specific cardinal categories and rejects a missing Arabic branch", () => {
        const reference = {
            amount_one: "{{count}} item",
            amount_other: "{{count}} items",
        };
        const arabic = {
            amount_zero: "{{count}} عناصر",
            amount_one: "{{count}} عنصر",
            amount_two: "{{count}} عنصران",
            amount_few: "{{count}} عناصر",
            amount_many: "{{count}} عنصرًا",
            amount_other: "{{count}} عنصر",
        };
        const plurals = [
            {
                path: "amount",
                kind: "cardinal" as const,
            },
        ];
        expect(checkCatalog(reference, arabic, "ar", plurals, [])).toEqual([]);
        const { amount_two: _two, ...broken } = arabic;
        expect(checkCatalog(reference, broken, "ar", plurals, []).join(" ")).toContain("two");
    });
    it("rejects malformed, missing, duplicate and foreign rich slots", () => {
        const reference = {
            sentence: "[[providerLink]] uses [[modelId]].",
        };
        const slots = [
            {
                path: "sentence",
                slots: [
                    "providerLink",
                    "modelId",
                ],
            },
        ];
        expect(
            checkCatalog(
                reference,
                {
                    sentence: "[[modelId]]：[[providerLink]]",
                },
                "zh-CN",
                [],
                slots,
            ),
        ).toEqual([]);
        for (const sentence of [
            "[[providerLink]]",
            "[[providerLink]] [[modelId]] [[modelId]]",
            "[[providerLink]] [[other]]",
            "[[providerLink]] [[modelId attr]]",
        ]) {
            expect(
                checkCatalog(
                    reference,
                    {
                        sentence,
                    },
                    "zh-CN",
                    [],
                    slots,
                ).length,
            ).toBeGreaterThan(0);
        }
    });
    it("validates ordinal categories against each locale's plural contract", () => {
        const ordinal = [
            {
                path: "rank",
                kind: "ordinal" as const,
            },
        ];
        const en = {
            rank_ordinal_one: "{{count}}st",
            rank_ordinal_two: "{{count}}nd",
            rank_ordinal_few: "{{count}}rd",
            rank_ordinal_other: "{{count}}th",
        };
        expect(
            checkCatalog(
                en,
                {
                    rank_ordinal_other: "第 {{count}} 名",
                },
                "zh-CN",
                ordinal,
                [],
            ),
        ).toEqual([]);
    });
    it("rejects declared plural paths that are absent from the semantic inventory", () => {
        expect(
            checkCatalog(
                {
                    hello: "Hello",
                },
                {
                    hello: "你好",
                },
                "zh-CN",
                [
                    {
                        path: "missing",
                        kind: "cardinal",
                    },
                ],
                [],
            ).length,
        ).toBeGreaterThan(0);
    });
});
