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

    // The dev server listens on a fixed high port to avoid collisions with
    // the Vite defaults (5173/5174) commonly used by other projects.
    server: {
        port: 56625,
        strictPort: true,
    },

    build: {
        outDir: "../target/web/dist",
        emptyOutDir: true,
    },
});
