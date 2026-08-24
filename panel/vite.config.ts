import solid from "@solidjs/vite-plugin";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";

export default defineConfig({
    root: "panel",
    resolve: {
        tsconfigPaths: true,
    },
    plugins: [
        solid(),
        tailwindcss(),
    ],
    build: {
        outDir: "../dist",
        emptyOutDir: true,
    },
});
