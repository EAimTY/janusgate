This file applies to the entire repository.

This repository is JanusGate. It is a self-hosted, lightweight AI gateway written in Rust with an in-process SQLite-compatible database (Turso). The repository also hosts the gateway's administration frontend as the `web/` workspace (janusgate-web, a statically-hosted SPA over the management API); its working rules live in `web/AGENTS.md`.

## Global Working Rules

### Language-agnostic

- English is the repository's written language: documentation, code, and comments always use it. User-facing UI text in another language is deliberate and stays untranslated.
- Read `README.md` before starting non-trivial work. Treat `README.md` as the source of truth for the whole system's design and architecture. Follow a documentation-first approach: update documentation alongside every change.
- Be strict. Avoid fuzzy, ambiguous, or weakly specified behavior; make semantics explicit.
- Do not preserve backward compatibility in code. This project is new; implement the design in the most elegant and idiomatic form available. Avoid historical debt.
- Name booleans with predicate-style name prefixes such as `is`, `can`, `has`, or `should`.
- Prefer unabbreviated names. For example, use `permanent_id` instead of `perm_id`. Exceptions: "ctx", "txn", "err", "repr", "src", "dst", "std", "id".
- Give every item the most restrictive visibility that permits its intended use.
- All code and configuration are first-party: the author is the only user and guarantees the configuration's validity. Trust them; never re-validate at runtime. Put constraints in the types that carry the values — `NonZero` integers, enums, serde's deserialization rejection — not in production asserts. A violation of a documented contract, such as a demand for a bar size the provider cannot stream, is fatal: never a runtime error event or a recovery path.
- Add comments for non-trivial or hard-to-understand logic.
- Keep folder-local `AGENTS.md` files focused on stable guidance. Do not include implementation details that must be updated after code changes.
- Follow test-driven development (TDD): write tests before implementing functionality.
- Quality gates are per-area: the Rust sources require the cargo matrix below, and the `web/` workspace's gates live in `web/AGENTS.md`.
- Do not write trivial tests: tests protect complex logic, invariants, edge cases, and failure-prone paths — never a ritual or a self-evident change (such as mechanically asserting a new field's presence or a removed field's absence). Keep coverage cohesive, not inflated: when functionality changes, update and consolidate existing tests instead of accumulating redundant ones. Test only first-party domain logic — never upstream dependencies, libraries, or standard-library primitives (string formatting, basic macro expansion) — unless explicitly relying on their fragile, unspecified external behavior. Apply the same restraint to TDD: write tests first only when designing non-trivial logic, not for boilerplate or compiler-guaranteed mechanics.

### Rust

- Declare every external dependency in `[dependencies]` with `default-features = false` and declare required features there.
- Model every JSON payload as a concrete type deriving `Serialize` and `Deserialize`; never handle JSON dynamically — no `serde_json::json!`, hand-built `Map`s, `.get("key")` walks, or `serde_json::Value`. `Value` is allowed only for fragments whose shape is not the gateway's to define, such as an enum's `Other` variant. Test code is exempt.
- Every API route declares its request and response payload types explicitly next to its handler; a route without a request body declares an empty concrete request type.
- Access configuration values directly from `crate::CONFIG` instead of threading them through function arguments.
- Configuration, contract, and invariant violations are fatal: use `unwrap()` / `expect()` instead of recovery paths that obscure bugs and complicate debugging.
- Do not contort idiomatic code into verbose workarounds merely to dodge panic-related Clippy lints (`unwrap_used`, `expect_used`, `panic`, `indexing_slicing`, `todo`, `unimplemented`, `unreachable`). Where panicking is safe and acceptable — a proven invariant or an intentional fail-fast — keep the direct construct and justify it with `#[expect(..., reason = "...")]` on the narrowest scope: a single statement or match arm, not a whole function or block.
- Rustdoc is mandatory for the crate and every item, private ones included; intra-doc links must resolve. Add `# Panics` and `# Safety` sections where the corresponding Clippy lints apply. Lint policy is configured centrally in `[lints]` in `Cargo.toml` and in `clippy.toml`.
- After every Rust code modification, run `cargo fmt --all`, `cargo check --workspace --all-targets`, `cargo clippy --workspace --all-targets`, and `cargo test --workspace`. Run each twice: once with default features (the shipped server configuration) and once with `--all-features`, so feature-gated code and `required-features` binaries are always covered.

### SQL (SQLite)

- The database schema lives in `src/database/schema/`. Every schema modification is a new `version_x.rs` module — hand-written SQL DDL whose only item is `const SCHEMA: Schema` — plus the migration text upgrading the previous version and a test applying the schema to a fresh database. Register the module in `SCHEMAS` in `src/database/schema/mod.rs`. Never edit an existing version module; existing versions are immutable history.
- Migrations run under foreign key enforcement and must never rely on disabling it: `PRAGMA foreign_keys` is a no-op inside a SQLite transaction, and Turso's mid-transaction acceptance of it is a divergence that may be aligned at any time. Author every migration to hold under enforcement statement by statement; order table-rebuild steps so referential integrity holds at every intermediate state.
- Explicitly declare nullability, uniqueness, and foreign keys.
- Reserve `CHECK` constraints for closed domain invariants (such as enum value sets); delegate all input and format validation to the application layer.
- Use `STRICT` in table definitions.
