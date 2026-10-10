# Management API working rules

Applies to `src/management_api/` and the `generate-management-api-typescript-bindings` binary, alongside the [global rules](../../AGENTS.md). Read the [management contract](../../docs/reference/management-api.md) and affected [resource schemas](../../docs/reference/management-api/README.md) before changes.

## Route and version ownership

- Each version owns its router, route registry, wire surface, errors, and logging classification. Share only version-independent machinery at the parent module. The version prefix owns mounting and output file naming.
- Declare each route exactly once beside its handlers through `route!`, with relative method/path and named concrete request/response types. Build routers and bindings from these declarations; never duplicate a method/path inventory. Empty requests and responses still have named concrete structs; `()` or inline-only payloads are invalid declarations.
- Publicly flatten the version's route markers, nested payload types, and error body for Rust consumers. Publish fields and derive both `Serialize` and `Deserialize`. Keep handlers and storage private. Share payload declarations by reference; names must be unique within a version.

## Wire boundaries

- Use a concrete error variant per identifiable failure, carrying the field, value, or entity identity needed for diagnosis. Derive client wording from the variant. Internal causes expose only the category's fixed message.
- Converge body, media type, size, authorization, routing, and handler failures into the version's error type. Wrapped JSON extraction and the error's response conversion own rejection normalization, rendering, and exactly-once logging; no framework rejection escapes.
- Preserve the [documented two-state write semantics](../../docs/reference/management-api.md#write-semantics). Absent-keeps fields use `absent_keeps`, serde default, and `ts(optional)`; explicit null is rejected. Required-nullable fields use `Nullable<T>` and an aligned TypeScript nullable annotation; omission tolerates null. Never introduce three distinct meanings for absent/null/value.
- Closed wire sets are unordered unions. Do not create an enum enumeration/order artifact in bindings.

## Generation and verification

- Keep registries, `ts_rs` derives, and generation behind `management-api-typescript-bindings`; the normal server must not depend on them. Emit one self-contained file per version, containing every declared payload, errors, and the typed route table, with no imports.
- serde defines serialization truth. Annotate custom serialization explicitly where automatic TypeScript derivation cannot reproduce it.
- Update the semantic contract, run the [Rust matrix](../../docs/development.md#changes-and-verification), regenerate the committed reference with `npm run docs:generate`, verify with `npm run docs:check`, and regenerate web bindings when its consumer changes.
