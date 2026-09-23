import solid from "@solidjs/vite-plugin";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";

export default defineConfig({
    resolve: {
        tsconfigPaths: true,
    },

    plugins: [
        solid(),
        tailwindcss(),
    ],

    // Generated web state lives under the repository's target/ tree, beside
    // the Rust build artifacts: the dev-server cache and the build output.
    cacheDir: "../target/web/cache",

    server: {
        port: 56625,
    },

    build: {
        outDir: "../target/web/dist",
        emptyOutDir: true,
    },
});
