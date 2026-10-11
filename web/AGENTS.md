# Console working rules

Applies to `web/` alongside the [global rules](../AGENTS.md). Read [README.md](README.md), [architecture](docs/architecture.md), and the affected [management contract](../docs/reference/management-api.md). For interface work also read [PRODUCT.md](../PRODUCT.md), [DESIGN.md](DESIGN.md), and the [surface brief](.impeccable/surfaces/web.md). Read [localization](docs/localization.md) when changing copy or language behavior.

## Documentation and boundaries

- Generated management bindings own wire shapes. Regenerate with `npm run generate-bindings -w web` from the repository root; do not edit or hand-copy the ignored generated files.
- Keep product intent, visual guidance, browser state contracts, and localization in their owning documents. Update the design sidecar whenever changing DESIGN.md; its tokens come from the frontmatter, and its narrative mirrors the Markdown.
- Preserve dedicated page ownership, explicit complete writes, stale-response guards, held dirty drafts, and locale-independent form lifetime. These contracts live in the architecture document rather than here.
- Put authored UI copy in all supported typed catalogs; preserve foreign messages and values. Run the catalog gate through the normal quality check.

## TypeScript and TSX

- Treat `/` as the `web/` root in import specifiers. Prefer root imports such as `/src/main` unless the imported module is in the same folder (`./module`) or recursively within a sibling folder (`../module` or `../folder/module`). If a relative import would climb to the `web/` root, use `/` instead. Generated files are exempt from this import rule.
- After every modification of this workspace's code, run `npm run check` and `npm run test -w web` from the repository root.
- Do not write trivial tests: vitest suites protect first-party logic — state machines, parsers, adapters, editing contracts, and component behavior — never library behavior or self-evident wiring. Keep coverage cohesive, not inflated.
- Generic parameter naming is all-or-nothing per list. When a generic list is simple—a few distinct parameters with no overlapping roles—use single-letter generics. Otherwise, once any parameter needs a descriptive multi-letter name, make every parameter in that list multi-letter; never mix single-letter and multi-letter generics in the same list. Multi-letter generics carry a role prefix: `TXxx` for a type parameter and `FXxx` for a function or callback parameter, where `Xxx` describes the parameter's use.
- Use strict types and narrow unknown input through explicit validation. Do not use `any` or unchecked type assertions to bypass the type system. Use explicit return types for exported functions and public component helpers.
