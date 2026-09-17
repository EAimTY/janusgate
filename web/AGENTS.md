This file applies to the entire `web/` workspace.

This workspace is janusgate-web, the standalone, statically-hosted web frontend for the JanusGate management API: on first visit it asks for a JanusGate management API base URL and token, then administers that JanusGate instance entirely through cross-origin `/management-api/v0/*` JSON calls. The management API's wire contract is not hand-written here — it is generated from the gateway sources at the repository root into a gitignored TypeScript file (`npm run generate-bindings`, run from `web/`), and application code compiles against it.

## Global Working Rules

### Language-agnostic

- English is the repository's written language: documentation, code, and comments always use it. User-facing UI text in another language is deliberate and stays untranslated.
- Read `README.md` before starting non-trivial work. Treat `README.md` as the source of truth for the whole system's design and architecture. Always follow a documentation-first approach when making any modifications, ensuring all documentation is updated alongside every change.
- Be strict. Avoid fuzzy, ambiguous, or weakly specified behavior; make semantics explicit.
- Do not preserve backward compatibility in code. This project is new; implement the design in the most elegant and idiomatic form available. Avoid historical debt.
- Name booleans with predicate-style name prefixes such as `is`, `can`, `has`, or `should`.
- Prefer unabbreviated names. For example, use `permanent_id` instead of `perm_id`. Exceptions: "ctx", "txn", "err", "repr", "src", "dst", "std", "id".
- Give every item the most restrictive visibility that permits its intended use.
- All code and configuration are first-party — the project's author is its only user and guarantees the configuration is valid — so trust them instead of re-validating at runtime. Constraints belong in the types that carry the values — `NonZero` integers, enums, serde's deserialization rejection — not in production asserts. A violation of a documented contract, such as a demand for a bar size the provider cannot stream, is fatal: never a runtime error event or a recovery path.
- Add comments for non-trivial or hard-to-understand logic.
- Keep folder-local `AGENTS.md` files focused on stable guidance. Do not include implementation details that must be updated after code changes.
- Every `AGENTS.md` must have a sibling `CLAUDE.md` containing exactly `@AGENTS.md`.
- Follow test-driven development (TDD): write tests before implementing functionality.
- Do not write trivial tests. Tests exist to protect complex logic, invariants, edge cases, and failure-prone paths, never to satisfy a ritual or verify self-evident modifications (such as mechanically asserting the presence or absence of a newly added or removed field). Keep coverage cohesive rather than inflating test counts: when functionality changes, update and consolidate existing tests where appropriate instead of accumulating redundant cases. Test only first-party domain logic; never test upstream dependencies, libraries, or standard-library primitives (such as string formatting or basic macro expansion) unless explicitly relying on fragile, unspecified external behavior. In TDD, apply the same restraint: write tests first only when designing non-trivial logic, not for boilerplate or compiler-guaranteed mechanics.

## TypeScript and TSX

- Treat `/` as the `web/` root in import specifiers. Prefer root imports such as `/src/main` unless the imported module is in the same folder (`./module`) or recursively within a sibling folder (`../module` or `../folder/module`). If a relative import would climb to the `web/` root, use `/` instead. Generated files are exempt from this import rule.
- After every modification of this workspace's code, run `npm run check` and `npm test` from `web/`.
- Generic parameter naming is all-or-nothing per list. When a generic list is simple—a few distinct parameters with no overlapping roles—use single-letter generics. Otherwise, once any parameter needs a descriptive multi-letter name, make every parameter in that list multi-letter; never mix single-letter and multi-letter generics in the same list. Multi-letter generics carry a role prefix: `TXxx` for a type parameter and `FXxx` for a function or callback parameter, where `Xxx` describes the parameter's use.
- Use strict types and narrow unknown input through explicit validation. Do not use `any` or unchecked type assertions to bypass the type system. Use explicit return types for exported functions and public component helpers.
