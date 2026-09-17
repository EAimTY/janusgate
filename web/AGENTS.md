This file applies to the entire `web/` workspace. The repository root's `AGENTS.md` applies unchanged alongside it; only the web-specific rules live here.

This workspace is janusgate-web, the standalone, statically-hosted web frontend for the JanusGate management API: on first visit it asks for a JanusGate management API base URL and token, then administers that JanusGate instance entirely through cross-origin `/management-api/v0/*` JSON calls. The management API's wire contract is not hand-written here — it is generated from the gateway sources at the repository root into a gitignored TypeScript file (`npm run generate-bindings`, run from `web/`), and application code compiles against it.

## TypeScript and TSX

- Treat `/` as the `web/` root in import specifiers. Prefer root imports such as `/src/main` unless the imported module is in the same folder (`./module`) or recursively within a sibling folder (`../module` or `../folder/module`). If a relative import would climb to the `web/` root, use `/` instead. Generated files are exempt from this import rule.
- After every modification of this workspace's code, run `npm run check` and `npm run test:web` from the repository root.
- Generic parameter naming is all-or-nothing per list. When a generic list is simple—a few distinct parameters with no overlapping roles—use single-letter generics. Otherwise, once any parameter needs a descriptive multi-letter name, make every parameter in that list multi-letter; never mix single-letter and multi-letter generics in the same list. Multi-letter generics carry a role prefix: `TXxx` for a type parameter and `FXxx` for a function or callback parameter, where `Xxx` describes the parameter's use.
- Use strict types and narrow unknown input through explicit validation. Do not use `any` or unchecked type assertions to bypass the type system. Use explicit return types for exported functions and public component helpers.
