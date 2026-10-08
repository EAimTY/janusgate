#!/usr/bin/env node
/**
 * The catalog gate: validates every shipped localization catalog against the
 * English canonical inventory and checks the registry's authoring invariants.
 *
 * The gate drives the development-only `src/i18n/catalog-check.ts` module,
 * which the production import graph never reaches. It runs through Vite's SSR
 * module loader so the TypeScript sources load with this workspace's own
 * resolution rules — no extra build step. A failure
 * prints every problem it found and exits nonzero.
 */
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer } from "vite";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");

/** Every shipped catalog, keyed by its registry tag. */
const shippedCatalogModules = new Map([
    [
        "en",
        "/src/i18n/catalogs/en.ts",
    ],
    [
        "zh-CN",
        "/src/i18n/catalogs/zh-CN.ts",
    ],
    [
        "zh-Hant",
        "/src/i18n/catalogs/zh-Hant.ts",
    ],
    [
        "ja",
        "/src/i18n/catalogs/ja.ts",
    ],
    [
        "ko",
        "/src/i18n/catalogs/ko.ts",
    ],
    [
        "es",
        "/src/i18n/catalogs/es.ts",
    ],
]);

const server = await createServer({
    root: packageRoot,
    appType: "custom",
    logLevel: "warn",
    server: {
        middlewareMode: true,
    },
});

/** Every problem the gate found, already named with its catalog tag. */
const failures = [];

try {
    const { checkCatalog, checkRegistry } = await server.ssrLoadModule(
        "/src/i18n/catalog-check.ts",
    );
    const { languageDefaults, localeRegistry } =
        await server.ssrLoadModule("/src/i18n/registry.ts");

    // English is the canonical inventory every other catalog is compared against.
    const english = (await server.ssrLoadModule(shippedCatalogModules.get("en"))).default;

    for (const [tag, modulePath] of shippedCatalogModules) {
        const entry = localeRegistry[tag];
        if (entry === undefined) {
            failures.push(`${tag}: missing from the locale registry`);
            continue;
        }

        // The registry's own loader and the catalog module must be the same object:
        // a loader pointing at the wrong catalog would otherwise pass unnoticed.
        const catalog = (await server.ssrLoadModule(modulePath)).default;
        if ((await entry.load()) !== catalog)
            failures.push(`${tag}: the registry loader resolves a different catalog`);

        for (const problem of checkCatalog(english, catalog, tag))
            failures.push(`${tag}: ${problem}`);
    }

    for (const problem of await checkRegistry(localeRegistry, languageDefaults))
        failures.push(problem);
} finally {
    await server.close();
}

if (failures.length > 0) {
    for (const failure of failures) console.error(failure);
    console.error(`Catalog gate failed with ${failures.length} problem(s).`);
    process.exit(1);
}

console.log(`Catalog gate passed for ${shippedCatalogModules.size} catalogs.`);
