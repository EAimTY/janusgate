import solid from "@solidjs/vite-plugin";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig, normalizePath, type Plugin } from "vite";

// Relocating the dep cache below node_modules (cacheDir) takes the
// pre-bundled dependency files off the node_modules path Vite exempts from
// its dynamic-import analysis, so computed dynamic imports inside them —
// such as @solidjs/web's hydration module loader — raise the
// vite:import-analysis warning. Apply Vite's own documented suppression:
// mark every computed dynamic import in an optimized dependency as intended
// to be left as-is. The marker is an inert comment; the served code is
// otherwise untouched.
const computedDynamicImport = /import\(\s*(?!["']|`(?![^`]*\$\{))/g;

function ignoreComputedDynamicImportsInOptimizedDeps(): Plugin {
    // Vite identifies optimized dependency files by this path prefix alone,
    // so the same prefix check stands in for its internal depsOptimizer
    // lookup, which the public Environment type does not expose.
    let depsCacheDirPrefix = "";
    return {
        name: "janusgate:ignore-computed-dynamic-imports",
        apply: "serve",
        configResolved(config) {
            depsCacheDirPrefix = `${normalizePath(config.cacheDir)}/deps/`;
        },
        transform(code, id) {
            if (!normalizePath(id).startsWith(depsCacheDirPrefix)) return null;
            // match and replace reset the global regex's lastIndex, so the
            // shared constant carries no state between transforms.
            if (!code.match(computedDynamicImport)) return null;
            return {
                code: code.replace(computedDynamicImport, "import(/* @vite-ignore */ "),
                map: null,
            };
        },
    };
}

export default defineConfig({
    resolve: {
        tsconfigPaths: true,
    },

    plugins: [
        solid(),
        tailwindcss(),
        ignoreComputedDynamicImportsInOptimizedDeps(),
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
