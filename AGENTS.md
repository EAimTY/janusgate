This file applies to the entire repository.

This repository is JanusGate. It is a self-hosted, lightweight AI gateway written in Rust with an in-process SQLite-compatible database (Turso).

## Global Working Rules

### Language-agnostic

- Think and respond in English.
- Read `README.md` before starting non-trivial work. Treat `README.md` as the source of truth for the whole system's design and architecture. Always follow a documentation-first approach when making any modifications, ensuring all documentation is updated alongside every change.
- Be strict. Avoid fuzzy, ambiguous, or weakly specified behavior; make semantics explicit.
- Do not preserve backward compatibility in code. This project is new; implement the design in the most elegant and idiomatic form available. Avoid historical debt.
- The crates' `version` fields stay pinned at `0.0.0`: the unreleased project carries no release numbering, so never bump them.
- Name booleans with predicate-style name prefixes such as `is`, `can`, `has`, or `should`.
- Prefer unabbreviated names. For example, use `permanent_id` instead of `perm_id`. Exceptions: "ctx", "txn", "err", "repr", "src", "dst", "std".
- Give every item the most restrictive visibility that permits its intended use.
- Treat fatal configuration, contract and invariant violations as fatal instead of adding recovery paths that obscure bugs and complicate debugging.
- Add comments for non-trivial or hard-to-understand logic.
- Keep folder-local `AGENTS.md` files focused on stable guidance. Do not include implementation details that must be updated after code changes.
- Every `AGENTS.md` must have a sibling `CLAUDE.md` containing exactly `@AGENTS.md`.
- Follow test-driven development (TDD): write tests before implementing functionality.

### Rust

- Declare every external dependency in `[dependencies]` with `default-features = false` and declare required features there.
- Every place could trigger a panic explicitly should be explained with a comment `// SAFETY: reason`.

### SQL (SQLite)

- Treat `DATABASE.sqlite` as the only source of truth for the SQLite schema.
- Explicitly declare nullability, uniqueness, and foreign keys.
- Reserve `CHECK` constraints for closed domain invariants (such as enum value sets); delegate all input and format validation to the application layer.
- Use `STRICT` in table definitions.
