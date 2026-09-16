This file applies to the entire repository.

This repository is JanusGate. It is a self-hosted, lightweight AI gateway written in Rust with an in-process SQLite-compatible database (Turso).

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

### Rust

- Declare every external dependency in `[dependencies]` with `default-features = false` and declare required features there.
- Model every JSON payload as a concrete type deriving `Serialize` and `Deserialize`; never assemble or navigate JSON ad hoc — no `serde_json::json!`, no hand-built `Map`, no `.get("key")` walks. A `serde_json::Value` carries only fragments whose shape is not the gateway's to define. Test code is exempt: fixtures and wire-shape expectations may be raw JSON literals.
- Access configuration values directly from `crate::CONFIG` instead of threading them through function arguments.
- Treat fatal configuration, contract and invariant violations as fatal: use `unwrap()` / `expect()` instead of adding recovery paths that obscure bugs and complicate debugging.
- Do not contort idiomatic code into verbose workarounds merely to dodge panic-related Clippy lints (such as `unwrap_used`, `expect_used`, `panic`, `indexing_slicing`, `todo`, `unimplemented`, or `unreachable`). When panicking is safe and acceptable—such as a proven invariant or an intentional fail-fast fatal condition—retain the direct construct and document the justification using `#[expect(..., reason = "...")]` on the narrowest applicable scope (such as a single statement or match arm, rather than a whole function or block).
- Rustdoc is mandatory: every item, private ones included, and the crate itself must be documented, intra-doc links must resolve, and `# Panics` and `# Safety` sections are required where the corresponding Clippy lints apply. Lint policy is configured centrally in `[lints]` in `Cargo.toml` and in `clippy.toml`.
- After every code modification, run `cargo fmt --all`, `cargo check --workspace`, `cargo clippy --workspace`, and `cargo test --workspace` to ensure no formatting issues, compilation errors, lint regressions, or test failures were introduced.

### SQL (SQLite)

- Treat `src/database/SCHEMA.sqlite` as the only source of truth for the SQLite schema.
- Explicitly declare nullability, uniqueness, and foreign keys.
- Reserve `CHECK` constraints for closed domain invariants (such as enum value sets); delegate all input and format validation to the application layer.
- Use `STRICT` in table definitions.
