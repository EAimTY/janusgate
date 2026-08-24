import { cloudflareTest } from "@cloudflare/vitest-plugin";
import { defineConfig } from "vitest/config";

export default defineConfig({
    plugins: [
        cloudflareTest({
            wrangler: {
                configPath: "./wrangler.jsonc",
            },
            miniflare: {
                bindings: {
                    API_KEY_HASH: "",
                    CREDENTIALS_AES_KEY: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
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
