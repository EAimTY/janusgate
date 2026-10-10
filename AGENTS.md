# Repository working rules

Applies to the entire JanusGate repository. JanusGate is a self-hosted Rust AI gateway with in-process Turso storage and a separately hosted administration SPA in `web/`.

## Read by task

Start with [README.md](README.md) and the [documentation map](docs/README.md). Read only the contracts needed for the task, following their links when crossing domain boundaries.

| Area | Authoritative reading |
| --- | --- |
| Overall design and persistence | [Architecture](docs/design/architecture.md), [storage](docs/design/storage.md) |
| Accounts and routing | [Provider definitions](docs/design/providers.md), [routing and quota](docs/design/routing.md) |
| Protocols and compaction | [Inference interface](docs/reference/inference.md), [compaction](docs/design/compaction.md) |
| Management | [Management contract](docs/reference/management-api.md), [generated schemas](docs/reference/management-api/README.md), [local rules](src/management_api/AGENTS.md) |
| Configuration and tooling | [Configuration](docs/reference/configuration.md), [development](docs/development.md) |
| Frontend | [Web README](web/README.md), [local rules](web/AGENTS.md), [product](PRODUCT.md), [design](web/DESIGN.md) |

`AGENTS.md` owns working guidance, not system behavior or implementation inventories. Keep a contract in one authoritative document and link to it from other contexts.

## Shared authoring rules

- English is the repository's written language: documentation, code, and comments always use it. User-facing UI text in another language is deliberate and stays untranslated.
- In document files, insert line breaks only where the break itself carries meaning, never to limit line length.
- Read `README.md` before non-trivial work, then follow `docs/README.md` to the owning design or interface contract. The root README is the source-of-truth entry point; linked contracts supply the normative detail. Follow a documentation-first approach: update the owning documentation alongside every change, and regenerate interface references when payloads change.
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

## Rust

- Declare every external dependency in `[dependencies]` with `default-features = false` and declare required features there.
- Model every JSON payload as a concrete type deriving `Serialize` and `Deserialize`; never handle JSON dynamically — no `serde_json::json!`, hand-built `Map`s, `.get("key")` walks, or `serde_json::Value`. `Value` is allowed only for fragments whose shape is not the gateway's to define, such as an enum's `Other` variant.
- Every API route declares its request and response payload types explicitly next to its handler; a route without a request body declares an empty concrete request type.
- Access configuration values directly from `crate::CONFIG` instead of threading them through function arguments.
- Configuration, contract, and invariant violations are fatal: use `unwrap()` / `expect()` instead of recovery paths that obscure bugs and complicate debugging.
- Do not contort idiomatic code into verbose workarounds merely to dodge panic-related Clippy lints (`unwrap_used`, `expect_used`, `panic`, `indexing_slicing`, `todo`, `unimplemented`, `unreachable`). Where panicking is safe and acceptable — a proven invariant or an intentional fail-fast — keep the direct construct and justify it with `#[expect(..., reason = "...")]` on the narrowest scope: a single statement or match arm, not a whole function or block.
- Do not box enum variants solely to satisfy `large_enum_variant`. When inline size does not affect hot-path allocation or storage, keep the concrete variant inline and use `#[allow(clippy::large_enum_variant, reason = "...")]` on that enum. Apply the same rule when revisiting existing enums; retain indirection required by recursion, dynamic dispatch, or hot-path layout.
- Rustdoc is mandatory for the crate and every item, private ones included; intra-doc links must resolve. Add `# Panics` and `# Safety` sections where the corresponding Clippy lints apply. Lint policy is configured centrally in `[lints]` in `Cargo.toml` and in `clippy.toml`.
- After every Rust code modification, run `cargo fmt --all`, `cargo check --workspace --all-targets`, and `cargo clippy --workspace --all-targets`. Run each twice: once with default features (the shipped server configuration) and once with `--all-features`, so feature-gated code and `required-features` binaries are always covered.

## SQL (SQLite)

- The database schema lives in `src/database/schema/`. Every schema modification is a new `version_x.rs` module — hand-written SQL DDL whose only item is `const SCHEMA: Schema` — plus the migration text upgrading the previous version. Register the module in `SCHEMAS` in `src/database/schema/mod.rs`. Never edit an existing version module; existing versions are immutable history.
- Migrations run under foreign key enforcement and must never rely on disabling it: `PRAGMA foreign_keys` is a no-op inside a SQLite transaction, and Turso's mid-transaction acceptance of it is a divergence that may be aligned at any time. Author every migration to hold under enforcement statement by statement; order table-rebuild steps so referential integrity holds at every intermediate state.
- Explicitly declare nullability, uniqueness, and foreign keys.
- Keep schema constraints limited to SQLite storage types (`STRICT`), nullability, primary/unique/foreign keys, closed text enums, numeric ranges (including `0`/`1` for integer-backed booleans), JSON syntax validity (`json_valid` only), and invariants involving multiple columns.
- Do not add single-column text value or format checks such as non-empty strings, printable-character rules, or `GLOB` patterns. Enforce those contracts in Rust types, serde deserialization, and the application write or ingestion boundary instead.
- Use `STRICT` in table definitions.
