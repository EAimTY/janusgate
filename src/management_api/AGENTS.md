# Management API

This file applies to `src/management_api/` and the
`generate-management-api-typescript-bindings` binary.

- Every management API route is declared exactly once, next to its handlers, through the `route!` macro (method, path, request type, response type). The router is wired from those `Route` constants; a method or path string is never written anywhere else.
- The module publicly exposes the management API's full wire surface — the `Route` trait, every route marker, and every payload type down to nested rows, captured credentials, and the reasoning-effort enum — flattened at `management_api`, with public fields and both `Serialize` and `Deserialize`, so external Rust frontends can construct requests and parse responses. Handlers and storage stay private.
- Every request and response payload type is named and unique across the management API. A route that carries no request body still declares an empty request struct; `()` is never used as a payload type. Shared shapes (the provider and model envelopes) are shared by reference, not duplicated.
- serde attributes define the wire truth, and the feature-gated `ts_rs` derives must stay aligned with them: request fields that serde reads as optional carry `#[ts(optional = nullable)]`, and manually serialized payload types annotate their wire names explicitly.
- All TypeScript bindings machinery (the linkme route registry, the `ts_rs` derives, the generator) lives behind the `management-api-typescript-bindings` feature; the normal server build must not depend on it.
