# Provider definitions

A provider definition is an immutable connection recipe for one of five upstream formats. It separates wire format, connection data, and account credentials. The backend has no product-specific branches or seeded definitions. A provider binds an exact `(name, version)`; updating a definition appends its successor and does not move existing providers.

The [generated definition schemas](../reference/management-api/provider-definitions.md) specify every JSON field and tagged alternative. This document owns their interpretation and write-time assumptions. Unknown fields are rejected. Optional descriptor fields may accept null/defaults where their concrete serde type allows it; they do not introduce absent/null/value update semantics.

## Expressions and request templates

`StringExpr` is an ordered array of `ValueExpr` objects concatenated into one string. For example:

```json
[
  {"type":"literal","value":"Bearer "},
  {"type":"secret"}
]
```

Values include literal text, the current secret, named slots, the instance base URL, the exact upstream model ID, one UUID per inference dispatch, and stage-specific OAuth values. `coalesce` selects the first nonempty candidate and cannot nest another coalesce. A missing slot evaluates to empty text. Each stage admits only values available there; definition validation rejects unsatisfiable references before storage. A stored invariant violation is fatal.

`TemplateNode` is literal JSON, an evaluated string, an object, an array, or the concrete codec `body`. Inference envelopes require exactly one body placeholder; control requests forbid it. Object members are name-keyed and lexicographically rendered. `response_unwrap` selects an object member before parsing JSON documents or SSE frames; absence parses the original document.

Header names and authored header-value fragments must be valid HTTP header bytes. Query names/values use RFC 3986 percent encoding, with additions placed before a URL fragment. Form bodies use form-urlencoded encoding. Instance base URLs require `http://` or `https://`; machine-consumed keys and addresses are nonempty printable ASCII without whitespace. Model names additionally exclude `/` and `:`. Human labels are nonempty but otherwise free-form.

### Inference

`inference` declares URL, optional streaming URL, headers, upstream streaming mode, render options, optional envelope/unwrap, and optional quota rejection rules. `always` uses SSE even for non-streaming clients; `match_client` follows the caller. This decision controls the body stream flag and parser, never a response header. The gateway supplies Accept from framing; authored headers may override it. `omit_max_output_tokens` is accepted only for Responses.

Each dispatch evaluates one request UUID across all its expressions. Standalone compact connections have independent URLs/framing; overlays replace header names case-insensitively. Normal inference and compaction use the same credential and dispatch boundary, with account commitment on successful headers.

### Control requests and responses

A `ControlRequest` declares GET or POST, URL, headers, query rows, and an optional form or JSON body. The executor supplies Content-Type from the body kind. OAuth, provisioning, discovery, and quota use the same bounded control-plane transport; redirects are not followed.

Response descriptors declare JSON/form encoding and object-member paths (`[]` addresses the root). Missing required fields fail the stage; optional absence leaves its target unchanged; present wrong shapes fail. Each target can be assigned once within one descriptor. String extraction also accepts numeric values as decimal text; integer extraction requires an integer. Form responses are flat. Token-error codes are parsed independently from JSON or form and compared case-insensitively to authored terminal codes.

## Credentials and OAuth

`credential.type` is `api_key` or `oauth` and must match the stored provider credential kind. Static credentials are written through provider creation/update. OAuth recipes declare client identity, optional client secret/lifetime, initial slot generators, grant, exchange, optional ID-token claim extraction, and refresh mode.

### Grants and flow lifetime

Authorization-code grants build response type, client ID, redirect URI, state, and optional S256 challenge; extra parameters cannot duplicate those builder-owned names. Exchange must reference the supplied code and redirect URI, and the verifier exactly when PKCE is S256. A recipe without a client secret must supply client ID to token exchange and grant refresh. References to an unconfigured client secret are invalid.

Device authorization requires code, user code, a verification URI, and positive lifetime/interval evidence. Missing interval defaults to five seconds. Missing expiry requires an explicit fallback. Exchange references the device code; public-client authorization references client ID. The gateway polls independently of the browser, respects pending/slow-down scheduling, and resumes pending flows after restart. A completed exchange is checkpointed before provisioning so recovery does not repeat a consumed grant.

Authorization-code flows expire after ten minutes and are consumed by their first valid completion attempt, even if exchange or commit fails. Empty/malformed codes and grant mismatch fail before consumption. Device completion leaves pending flows intact and consumes a terminal success/failure once. Terminal failure reports gateway-owned codes/messages, never upstream bodies. Reauthorization replaces the targeted provider in place; if that provider was deleted before completion, the flow fails rather than recreating it.

### Provisioning and renewal

An ordered provisioning sequence runs after exchange, optionally skipping a step when its named slot exists. Poll steps have explicit budgets, done/error paths, and result extractions. Extractions may set slots, credentials, base URL, or expiry; the provider changes only after all required steps succeed. A provisioning secret reference requires exchange to produce that secret.

Expiry resolves from provisioning first, then positive `expires_in`, then a future access-token JWT expiry, then an explicitly configured lifetime, otherwise null. No expiry means no scheduled refresh, not immediate expiry. Malformed consulted expiry evidence fails the producing operation.

Refresh is `grant`, `reprovision`, or `none`. Grant requires a guaranteed refresh-token source, an exchange using it, and a response mapping producing an access token; omitted token rotation keeps the stored refresh token. Reprovision requires an unconditional way to produce renewed expiry. Renewal uses a conditional account-owned lease: concurrent credential replacement, reauthorization, or a newer lease cannot be overwritten by an older result. Normal renewal preserves quota restrictions; explicit credential replacement/rebinding clears them.

## Discovery and declarations

Discovery is optional. It extracts IDs from an array or object keys and optionally walks token/cursor pages under an explicit page budget. Exhausting the budget with more pages pending fails the entire discovery. Invalid IDs, present wrong types, and an empty final inventory fail before storage.

Array discovery can extract reasoning levels, positive input/output token capacities, and Messages compaction support booleans. Missing/null facts remain unknown; present malformed facts fail the whole refresh. Reasoning levels outside the gateway vocabulary are skipped and valid sets canonicalized. Each refresh rewrites discovery-owned facts, while administrator overrides remain untouched. [Routing](routing.md#inventory-and-automatic-exposure) defines row transitions and exposure.

Compaction configuration declares strategy, positive summary/retention budgets, optional readable-native export, per-mechanism support/operations, and an optional Messages token counter. Defaults are `native_preferred`, 4096 summary tokens, 8192 retained tokens, no export, and unknown mechanisms. Responses mechanisms require a Responses definition; Messages mechanisms/counters require Messages. Supported standalone operations require their independent connection; supported Messages operations require beta overlays. Present operation data is validated even when its default is unknown because a model override can select it. [Compaction](compaction.md) owns execution and replay compatibility.

## Quota

A quota descriptor declares one JSON control request and ordered object/array/map extraction sources. Request expressions admit only literal, secret, slot, base, and flat coalesce. Each source/entry has a unique stable key and explicit requiredness, scope, measurement, period, reset, and routing applicability. Arrays require stable required item identities; maps use their member keys. Identity is an ordered string tuple. Duplicate observed tuples fail.

Extraction is atomic: optional absent/null data is omitted or unknown, but present malformed active fields fail the complete reading. A true unlimited flag bypasses measurement fields only, never scope, period, or reset validation. Measurements are unknown, used ratio, used/limit counter, remaining amount, or unlimited. Used values may exceed limits; remaining ratios/counters stay within their declared bounds. Numeric encodings are number, strict JSON numeric string, or either; finite nonnegative amounts and JavaScript-safe integer magnitudes/timestamps are required.

Durations are positive whole seconds or calendar months without guessed alignment. Resets are declared Unix units, RFC 3339 instants at microsecond precision, or dates without invented timezone. Past resets are valid observations but not proof of replenishment. Only future absolute reset evidence can schedule availability rechecks.

Quota rejection rules are ordered, named, nonempty recipes: non-2xx statuses, an exact case-sensitive string code at an object-member path, an ordered restriction key, and account-wide/requested-model applicability. The first matching rule wins. Complete valid JSON within 4096 bytes is required; status alone, body prose, truncated/invalid data, and transport failures never prove exhaustion. Only rejected generation/native-compaction dispatches classify quota, before native-unsupported classification. Counters and control requests do not.

Quota viewing renews credentials normally and sends one bounded control request with no retry. It does not write models, switches, observation history, or accounting. Disabled accounts remain queryable. Provider listings and console mounting never trigger quota reads. [Routing](routing.md#quota-evidence) owns durable reconciliation and cadence.

## Frontend presets

Presets live only in `web/src/presets/`, with exact connection data, pinned evidence, and verification status beside each recipe. They prefill an ordinary definition; changes append a version and require explicit account rebind/reauthorization. `live` requires verification against the real upstream, not documentation research. Compaction and quota evidence are per mechanism/capability; supported wire format alone proves neither. Do not infer a quota endpoint or native operation from a product name or append it to an inference URL.
