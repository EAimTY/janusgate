# Management API v0

The administrative API is mounted at `/management-api/v0` only when `management_api` is configured at startup. It has its own versioned routes, payloads, and error domain, independent of inference protocols. Exact methods, full paths, request/response fields, and nested schemas are published in the [generated resource reference](management-api/README.md).

## Transport

Every action is POST with `Content-Type: application/json`, `Authorization: <management-token>` verbatim, and a JSON object body. Do not add Bearer. No-argument actions send `{}`. Success is HTTP 200 JSON, including `{}` for empty results. Request bodies are capped at 2 MiB. Cookies are not used; CORS preflight runs before authorization.

```bash
curl http://127.0.0.1:4718/management-api/v0/providers:list \
  -H 'Authorization: YOUR_MANAGEMENT_TOKEN' \
  -H 'Content-Type: application/json' \
  -d '{}'
```

Unknown request fields, malformed JSON, unsupported fields/variants, and invalid boundary values are rejected. Entity references use `<entity>_id`; rows return their own `id`. Timestamps are UTC microseconds, except date-only quota facts. Human names are nonempty; machine IDs/keys/base URLs follow the [provider boundary rules](../design/providers.md#expressions-and-request-templates).

### Errors

Inside v0, every failure is `{errno: number, message: string}` with exactly the associated HTTP status. Internal causes stay in logs; upstream management failures are 502, with safe context/foreign status in their message rather than a mirrored status or raw body.

| errno | Category | HTTP |
| --- | --- | --- |
| 1 | invalid_request | 400 |
| 2 | unauthorized | 401 |
| 3 | not_found | 404 |
| 4 | conflict | 409 |
| 5 | upstream_error | 502 |
| 6 | internal | 500 |
| 7 | payload_too_large | 413 |
| 8 | method_not_allowed | 405 |
| 9 | unsupported_media_type | 415 |

Authentication precedes unmatched v0 paths/methods. Outside every version, management/root fallbacks return empty 404s rather than errno JSON. Wrong methods on known paths include Allow. Missing/non-JSON Content-Type is 415.

## Write semantics

Updates state the complete desired readable/writable state, not patches. Required fields cannot be omitted to keep old values. Required-nullable mirror fields use `T | null`; omission deserializes as null and means no value, not keep. Client types require these keys. Optional write-only `providers:update.api_key` keeps the secret when omitted and replaces it when present; explicit null is rejected. Serving-key secrets are readable and always restated on update.

Optional rebind/filter/pagination fields accept omission but reject null. There is no field with three distinct update meanings for omission, null, and value. Optional descriptor sections and their defaults have the own serialized shapes shown in generated schemas; null/omission there denotes no configured value, not a partial-update instruction.

Variants use explicit tags, never inferred combinations of optional fields. Resource mutations validate references, uniqueness, and related state atomically. Responses supply server identities, server-minted values, or fresh state the caller cannot derive; writes that add no such facts answer `{}`. Readiness and permission previews derived from listed rows belong to clients.

## Provider definitions and accounts

[Definition routes and schemas](management-api/provider-definitions.md) list every stored version by name/version, create version 1, and append a successor using `base_version`. A stale base conflicts; no in-place editing/deletion exists. Optional definition sections configure provisioning, discovery, compaction, and quota. Their defaults and validation are specified in [provider definitions](../design/providers.md).

[Provider routes and schemas](management-api/providers.md) list accounts by name, create static-key accounts, rewrite fields/rebind a definition, delete accounts, query quota, read stored availability, and reset restrictions. Rows include exact definition identity, format, prefix, nullable base URL, and OAuth expiry where applicable; credentials are not returned.

Creation requires a static-key definition and its full instance state. An optional update rebind pair must be supplied together; a credential-kind change requires suitable new credentials. A static-key replacement can convert an OAuth account onto a static definition; OAuth acquisition uses reauthorization. Base URL must satisfy the effective definition's base references. Rename/switch edits preserve inventory. Deletion cascades account-owned inventory, direct routes, memberships, restrictions, and affected permissions; historical usage remains.

Quota viewing rejects unknown/unsupported providers before I/O, permits disabled accounts, forces a current-generation read, and reconciles durable availability. Availability reads never poll upstream. Reset clears stored restrictions and grants automatic-refresh grace without changing upstream allowance. [Routing](../design/routing.md#quota-evidence) defines reconciliation; [provider recipes](../design/providers.md#quota) define measurement semantics.

## OAuth

[OAuth routes and schemas](management-api/oauth.md) start creation, reauthorize an existing provider, and complete/read a flow. Start results are grant-tagged: authorization URL/state/expiry, or device state/expiry/user code/verification URIs. Completion is grant-tagged input and status-tagged output: pending, succeeded with provider, or failed with gateway-owned code/message.

Start requires exact definition identity, desired name/prefix, and base URL when referenced. Reauthorization retains account identity and prefix and optionally rebinds an OAuth definition; the optional pair must be supplied together. Authorization-code completion consumes one valid state attempt; a failed exchange needs a new flow. Device polling lives in the gateway; completion reads pending without consuming and consumes terminal outcomes once. Code-flow expiry is ten minutes; device expiry follows the recipe/response. Unknown, expired, or consumed states are 404. Full lifecycle and renewal assumptions live in [provider definitions](../design/providers.md#credentials-and-oauth).

## Models and public routes

[Physical-model schemas](management-api/models.md) list all rows, optionally filtering by provider, discover, create/claim, update switches/overrides, and delete. A filter with unknown provider ID yields an empty list, not not-found. Discovery/create/update return fresh rows; discovery never changes administrator overrides. Update cannot revive vanished state. Null overrides inherit; an empty reasoning-level override declares no levels; token overrides are positive integers. Compaction overrides name only the five known mechanisms and can explicitly suppress lower facts with unknown.

[Pool schemas](management-api/provider-pools.md) create/update complete name/prefix/switch/membership state atomically and list by name with members in priority order. Duplicate providers/priorities are client faults; missing providers are not-found. Deleting a pool referenced by an authored route conflicts.

[Public-route schemas](management-api/model-routes.md) list automatic and authored entries by public ID and create/update/delete aliases. Targets are `direct {model_id}` or `pool {pool_id, upstream_id}`. A pool route may name a currently absent upstream ID. Retarget preserves route identity and may keep its public name. Automatic route mutation conflicts; administer its provider/pool prefix and inventory instead. [Routing](../design/routing.md) owns exposure, availability, and pool commitment.

## Serving keys and access

[Serving-key schemas](management-api/api-keys.md) list by row ID, create, fully update, and delete. List/create return plaintext secrets because operators need to configure clients. Create/update require full `access_control`: key default, typed provider/pool defaults, and route overrides. Inherit is absence, not a wire decision.

Arrays are unordered on input and replaced atomically; duplicate typed owners or routes are 400, unknown references 404, and duplicate secrets 409 without secret disclosure. Explicit equal-default pins remain stored. Readable policies order provider defaults, pool defaults, and route overrides by ID. Disabled owners and owners with no routes are valid. New console drafts start deny with empty rules; API callers supply their own complete policy. Deletion revokes access and removes rules while preserving accounting. Permission resolution is specified in [routing](../design/routing.md#serving-key-permissions).

## Usage

[Usage schemas](management-api/usage.md) expose numerical attempts and aggregate buckets. Both reads share optional filters: time bounds, attribution IDs, endpoint, outcome, stream flag, and outward HTTP status. `from` is inclusive and `to` exclusive in UTC microseconds. Multi-value filters contain 1–100 values; within-field matches OR, fields combine AND. Unknown IDs match empty selections without lookup errors. Null fields are rejected; null recorded statuses cannot be selected by an HTTP-status array.

`usage:list.limit` defaults to 100 and must be 1–1000. `before` is an exclusive timestamp cursor; the client derives it from the last row. [Storage](../design/storage.md#usage-reads) documents boundary-tie and late-finalization limitations.

`usage:query.group_by` is a required duplicate-free set from `day`, `model`, `model_route`, `provider`, `api_key`, `endpoint_type`, `outcome`, `is_stream`, `http_status`. Empty grouping returns one total bucket even with no rows. Keys contain only requested dimensions; field serialization order is fixed, while bucket ordering follows requested dimensions with null first. Token sums ignore unknown measurements and render zero for all-null sums. Day is UTC `YYYY-MM-DD`. Reads remain available when recording is disabled; corrupt partitions fail explicitly. Labels come from current entity lists, with deleted IDs remaining valid historical attribution.

## Extension and generated bindings

Declare each route's method, relative path, and concrete request/response types beside its handler. Each version owns its error domain. The bindings feature emits one self-contained TypeScript file per version, including errors and a typed route table; no field/path inventory is maintained separately by the console.

`npm run docs:generate` rebuilds the committed schema reference from current Rust sources. `npm run docs:check` verifies it without writing. `npm run generate-bindings -w web` rebuilds the console's ignored runtime bindings. A schema change, behavior contract, and consumer adaptation belong in the same change. Bindings use serde-aware declarations; custom serialization changing a published shape needs an explicit aligned TypeScript annotation.
