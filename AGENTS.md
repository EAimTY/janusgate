This file applies to the entire repository.

## Global Working Rules

- Think and respond in English.
- Read `README.md` before starting non-trivial work. Treat `README.md` as the source of truth for the whole system's design and architecture.
- Be strict. Avoid fuzzy, ambiguous, or weakly specified behavior; make semantics explicit.
- Do not preserve backward compatibility. This project is new; implement the design in the most elegant and idiomatic form available. Avoid historical debt.
- Name booleans with predicate-style name prefixes such as `is`, `can`, `has`, or `should`.
- Prefer unabbreviated names. For example, use `permanent_id` instead of `perm_id`. Exceptions: "ctx", "txn", "err", "repr", "src", "dst", "std".
- Give every item the most restrictive visibility that permits its intended use.
- Treat fatal configuration, contract and invariant violations as fatal instead of adding recovery paths that obscure bugs and complicate debugging.
- Add comments for non-trivial or hard-to-understand logic.
- Keep folder-local `AGENTS.md` files focused on stable guidance. Do not include implementation details that must be updated after code changes.
- Every `AGENTS.md` must have a sibling `CLAUDE.md` containing exactly `@AGENTS.md`.

## TypeScript and TSX

- Treat `/` as the repository root in import specifiers. Prefer root imports such as `/common/api` unless the imported module is in the same folder (`./module`) or recursively within a sibling folder (`../module` or `../folder/module`). If a relative import would climb to the repository root, use `/` instead. Generated files are exempt from this import rule.
- Generic parameter naming is all-or-nothing per list. When a generic list is simple—a few distinct parameters with no overlapping roles—use single-letter generics. Otherwise, once any parameter needs a descriptive multi-letter name, make every parameter in that list multi-letter; never mix single-letter and multi-letter generics in the same list. Multi-letter generics carry a role prefix: `TXxx` for a type parameter and `FXxx` for a function or callback parameter, where `Xxx` describes the parameter's use.
- Use strict types and narrow unknown input through explicit validation. Do not use `any` or unchecked type assertions to bypass the type system. Use explicit return types for exported functions and public component helpers.

## SQL (SQLite)

- Treat `D1.sql` as the only source of truth for the D1 schema.
- Write SQL as strictly and explicitly as possible.
- Indent SQL with four spaces per nesting level.
- Use UPPERCASE for SQL keywords, PascalCase for type names, and snake_case for column names.
- Explicitly declare columns as nullable or non-null and define uniqueness and foreign keys. Add `CHECK` constraints only when permitted by `README.md`; JanusGate intentionally keeps human-input validation in the application.
- Use `STRICT` in table definitions.
