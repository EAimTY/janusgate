import { describe, expect, it } from "vitest";
import { checkCatalog, checkRegistry } from "./catalog-check";
import english from "./catalogs/en";
import chinese from "./catalogs/zh-CN";
import { languageDefaults, localeRegistry } from "./registry";

describe("production catalog contracts", () => {
    it("validates all shipped resources and registry metadata", async () => {
        expect(checkCatalog(english, english, "en")).toEqual([]);
        expect(checkCatalog(english, chinese, "zh-CN")).toEqual([]);
        expect(await checkRegistry(localeRegistry, languageDefaults)).toEqual([]);
        expect(await localeRegistry.en.load()).toBe(english);
        expect(await localeRegistry["zh-CN"].load()).toBe(chinese);
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
    it("validates ordinal categories and explicit zero contracts in every catalog", () => {
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
        const zero = [
            {
                path: "items",
                kind: "cardinal" as const,
                hasExplicitZero: true,
            },
        ];
        const reference = {
            items_zero: "No items for {{owner}}",
            items_one: "{{count}} item for {{owner}}",
            items_other: "{{count}} items for {{owner}}",
        };
        const translated = {
            items_zero: "{{owner}} 没有条目",
            items_other: "{{owner}} 有 {{count}} 个条目",
        };
        expect(checkCatalog(reference, translated, "zh-CN", zero, [])).toEqual([]);
        expect(
            checkCatalog(
                reference,
                {
                    ...translated,
                    items_zero: "没有条目",
                },
                "zh-CN",
                zero,
                [],
            ).length,
        ).toBeGreaterThan(0);
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
