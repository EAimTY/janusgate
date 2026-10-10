import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// The route registry and serde-aware bindings own wire shapes. This projection
// removes implementation commentary and assigns each declaration one document.
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const output = join(root, "docs/reference/management-api");
const isCheck = process.argv.includes("--check");
const temporary = mkdtempSync(join(tmpdir(), "janusgate-reference-"));
const titles = {
    "provider-definitions": "Provider definitions",
    providers: "Providers, quota, and availability",
    oauth: "OAuth flows",
    models: "Physical models",
    "provider-pools": "Provider pools",
    "model-routes": "Public model routes",
    "api-keys": "Serving keys and model access",
    usage: "Request usage",
    common: "Shared payload types",
};

try {
    const generation = spawnSync(
        "cargo",
        [
            "run",
            "--quiet",
            "--locked",
            "--bin",
            "generate-management-api-typescript-bindings",
            "--features",
            "management-api-typescript-bindings",
            "--",
            temporary,
        ],
        {
            cwd: root,
            stdio: "inherit",
        },
    );
    if (generation.status !== 0) throw new Error("Bindings generation failed");

    const rawSource = readFileSync(join(temporary, "v0.ts"), "utf8")
        .replace(/\/\*[\s\S]*?\*\//g, "")
        .replace(/\n\s*\n/g, "\n");
    const formatting = spawnSync(
        join(root, "node_modules/.bin/biome"),
        [
            "format",
            "--stdin-file-path=reference.ts",
        ],
        {
            cwd: root,
            input: rawSource,
            encoding: "utf8",
        },
    );
    if (formatting.status !== 0) throw new Error(`Schema formatting failed: ${formatting.stderr}`);
    const source = formatting.stdout;
    const declarations = new Map();
    for (const match of source.matchAll(
        /^export (?:type|enum) (\w+)[\s\S]*?(?=^export |$(?![\s\S]))/gm,
    )) {
        if (
            [
                "Routes",
                "Path",
                "Method",
                "Request",
                "Response",
            ].includes(match[1])
        )
            continue;
        declarations.set(match[1], match[0].trim());
    }
    const routes = Array.from(
        source.matchAll(/"(\/[^"\n]+)":\s*\{\s*(\w+):\s*\{\s*request: (\w+);\s*response: (\w+);/g),
        ([, path, method, request, response]) => ({
            path,
            method,
            request,
            response,
            group: path.slice(1).split(":")[0],
        }),
    );
    if (routes.length === 0 || declarations.size === 0)
        throw new Error("Generated reference is empty");
    for (const route of routes) {
        if (!Object.hasOwn(titles, route.group))
            throw new Error(`Add a reference group for ${route.group}`);
    }

    // Ignore quoted wire strings when finding references between declarations.
    const dependencies = (name) =>
        new Set(
            Array.from(
                declarations
                    .get(name)
                    .replace(/"[^"\n]*"/g, "")
                    .matchAll(/\b[A-Za-z]\w*\b/g),
                ([word]) => word,
            ).filter((word) => word !== name && declarations.has(word)),
        );
    const consumers = new Map();
    function visit(name, group) {
        const groups = consumers.get(name) ?? new Set();
        if (groups.has(group)) return;
        groups.add(group);
        consumers.set(name, groups);
        for (const dependency of dependencies(name)) visit(dependency, group);
    }
    for (const route of routes) {
        visit(route.request, route.group);
        visit(route.response, route.group);
    }
    visit("ErrorCode", "common");
    visit("ErrorResponse", "common");
    const owners = new Map(
        Array.from(consumers, ([name, groups]) => [
            name,
            groups.size === 1
                ? [
                      ...groups,
                  ][0]
                : "common",
        ]),
    );
    const link = (name, group) =>
        `[${name}](${owners.get(name) === group ? "" : `${owners.get(name)}.md`}#${name.toLowerCase()})`;
    const pages = new Map();
    for (const [group, title] of Object.entries(titles)) {
        const groupRoutes = routes.filter((route) => route.group === group);
        let content = `# ${title}\n\nGenerated from the Rust v0 route registry and serde-aware TypeScript bindings. Regenerate with \`npm run docs:generate\`; do not edit this page. Read the [management contract](../management-api.md) for transport, validation, defaults, and behavior.\n\n`;
        if (groupRoutes.length > 0) {
            content +=
                "## Routes\n\nAll paths below are complete. Success is HTTP 200 JSON.\n\n| Method | Path | Request | Response |\n| --- | --- | --- | --- |\n";
            for (const route of groupRoutes)
                content += `| \`${route.method}\` | \`/management-api/v0${route.path}\` | ${link(route.request, group)} | ${link(route.response, group)} |\n`;
            content += "\n";
        }
        content +=
            "## Payload schemas\n\nSchemas use TypeScript notation for JSON objects and unions. `?` means omission is allowed; `null` is allowed only where written. `Record<symbol, never>` denotes the empty JSON object `{}`. Numeric bounds and accepted input defaults are specified in the owning contract.\n\n";
        for (const [name, owner] of owners) {
            if (owner !== group) continue;
            const declaration = declarations.get(name).trim();
            content += `### ${name}\n\n\`\`\`typescript\n${declaration}\n\`\`\`\n\n`;
            const references = [
                ...dependencies(name),
            ];
            if (references.length > 0)
                content += `Related types: ${references.map((reference) => link(reference, group)).join(", ")}.\n\n`;
        }
        pages.set(`${group}.md`, content);
    }
    const index =
        "# Management API schemas\n\nGenerated method, path, request, response, and nested payload reference for `/management-api/v0`. Read the [management contract](../management-api.md) for semantics.\n\n" +
        Object.entries(titles)
            .map(([group, title]) => `- [${title}](${group}.md)`)
            .join("\n") +
        "\n\nRegenerate with `npm run docs:generate`; verify against the current Rust sources with `npm run docs:check`.\n";
    pages.set("README.md", index);
    const stale = [];
    for (const [filename, content] of pages) {
        const path = join(output, filename);
        if (isCheck) {
            let actual;
            try {
                actual = readFileSync(path, "utf8");
            } catch {
                actual = undefined;
            }
            if (actual !== content) stale.push(filename);
        } else {
            mkdirSync(output, {
                recursive: true,
            });
            writeFileSync(path, content);
        }
    }
    if (stale.length > 0)
        throw new Error(
            `Stale management reference: ${stale.join(", ")}. Run npm run docs:generate.`,
        );
    console.log(
        `${isCheck ? "Verified" : "Generated"} ${routes.length} routes and ${owners.size} payload types in ${pages.size} reference pages.`,
    );
} finally {
    rmSync(temporary, {
        recursive: true,
        force: true,
    });
}
