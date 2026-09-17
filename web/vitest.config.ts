import solid from "@solidjs/vite-plugin";
import { defineConfig } from "vitest/config";

export default defineConfig({
    resolve: {
        // Tests run the DOM-oriented application code, so resolve packages the
        // way the browser build does — including the development condition, so
        // Solid's development-time invariants are enforced in tests.
        conditions: [
            "browser",
            "development",
        ],
        tsconfigPaths: true,
    },
    cacheDir: "../target/web/cache",
    plugins: [
        solid(),
    ],
    test: {
        environment: "jsdom",
        include: [
            "**/*.test.ts",
            "**/*.test.tsx",
        ],
        coverage: {
            reportsDirectory: "../target/web/coverage",
        },
    },
});
