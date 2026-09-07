import { cloudflareTest } from "@cloudflare/vitest-plugin";
import { defineConfig } from "vitest/config";
// Relative (not the tsconfig "/" alias): Vite bundles this config file before path aliases exist.
import { TEST_CREDENTIALS_AES_KEY } from "./tests/test-secrets";

export default defineConfig({
    plugins: [
        cloudflareTest({
            wrangler: {
                configPath: "./wrangler.jsonc",
            },
            miniflare: {
                bindings: {
                    API_KEY_HASH: "",
                    CREDENTIALS_AES_KEY: TEST_CREDENTIALS_AES_KEY,
                    PANEL_TOKEN_HASH: "",
                },
            },
        }),
    ],
    test: {
        include: [
            "**/*.test.ts",
        ],
    },
});
