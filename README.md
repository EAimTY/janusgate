# JanusGate — OpenAI-Compatible Gateway (Rust)

> Single source of truth for the system design and architecture. Contracts and invariants stated here are **normative**.

---

## 1. Overview & Scope

**JanusGate** is a self-hosted, lightweight AI gateway written in Rust. It exposes an OpenAI-compatible API (`/api/openai-v1`) backed by heterogeneous upstream accounts, managed via a versioned management JSON API (`/management-api/v0/*`). The repository additionally hosts [`web/`](web/README.md) — janusgate-web, the standalone statically-hosted administration frontend for that management API; `web/README.md` is the source of truth for the frontend, and this document remains the source of truth for the gateway.

### Provider Types

| Type | Identity | Upstream Wire Format |
| :--- | :--- | :--- |
| `openai_response` | Static API Key | OpenAI Responses (`openai_response`) |
| `openai_chat_completion` | Static API Key | OpenAI Chat Completions (`openai_chat_completion`) |
| `gemini_generate_content` | Static API Key | Gemini GenerateContent (`gemini_generate_content`), bare request |
| `codex_oauth` | ChatGPT Account | OpenAI Responses (`openai_response`) via Codex endpoint |
| `antigravity_oauth` | Google Account | Gemini GenerateContent (`gemini_generate_content`) via Antigravity |

### Outward Protocols
- **Chat Completions**: `POST /api/openai-v1/chat/completions` (streaming SSE & non-streaming)
- **Responses**: `POST /api/openai-v1/responses` (streaming SSE & non-streaming)
- **Model Discovery**: `GET /api/openai-v1/models`

### Design Principles & Non-Goals
- **Stateless execution**: Zero persistence of prompts, completions, tokens, or usage metrics.
- **Headless server**: The gateway serves no frontend of its own — no SPA, no static assets. Administration is conducted exclusively through `/management-api/v0/*` JSON endpoints; the repository's `web/` frontend is a separately deployed static application that talks to those endpoints over the network and is never served by the gateway.
- **Strict model passthrough**: Zero automatic or hardcoded model mapping. Upstream calls pass the exact model identifiers yielded by upstream model discovery (`upstream_id`), even for OAuth providers, and reasoning effort values are forwarded verbatim — the gateway applies no mapping to them.
- **Excluded**: Multi-tenancy, dynamic pooling/load balancing, automatic provider fallback, automatic model mapping/aliasing, prompt caching, request logging, billing/rate-limiting, outbound proxying.

---

## 2. Technology Stack & Request Pipeline

### 2.1 Technology Stack

| Layer | Component | Notes |
| :--- | :--- | :--- |
| **Language** | Rust (2024 edition) | Strict types, memory safety, zero-cost abstractions |
| **HTTP Server** | `hserver` + `socket2` + `rustls` | The gateway's `server` module owns the TCP listener (`socket2`) and the accept loop; `hserver` drives the per-connection Hyper lifecycle — HTTP/1 or HTTP/2, auto-detected per connection — through one connection driver built at startup and shared by every accepted connection, wrapped in TLS (`rustls` via hserver's `tls` feature) when the `tls` configuration section is present (§8) |
| **Routing & Handlers** | `axum` (Router & utils) | Hierarchical routing (implements `tower::Service`), request extractors, and response helpers; `axum::serve` is strictly unused |
| **Middleware** | `tower` + `tower-http` | Service abstraction, middleware composition, and layer stacking; `tower-http` (cors feature) provides the CORS layer (§4) and the `AllowOrigin` type of the configuration schema (§8) |
| **Database** | `turso` | In-process SQLite-compatible SQL database (`tursodatabase/turso`) |
| **HTTP Client** | `reqwest` | Connection pooling and streaming |
| **Serialization** | `serde` + `serde_json` | Strongly typed parsing and serialization |
| **Logging** | `log` + `env_logger` | Log facade and stderr emission (§8); `env_logger` is built entirely from configuration, never from environment variables |
| **File Watching** | `notify` | Configuration hot-reload file watching (§8); the platform's native backend — inotify on Linux, FSEvents on macOS, ReadDirectoryChangesW on Windows — feeding a Tokio channel |
| **Async Runtime** | `tokio` | Multi-threaded async I/O |

### 2.2 Server Pipeline Architecture & Router Hierarchy

```text
[Incoming TCP]
               │
               ▼
      ┌─────────────────┐
      │server.rs+hserver│  (server.rs: TCP listener & accept loop; hserver: HTTP/1 & HTTP/2
      └────────┬────────┘   lifecycle; TLS handshake first when `tls` is configured)
               │ (invokes Router as tower::Service)
               ▼
      ┌─────────────────┐
      │   Root Router   │  (404: empty)
      └───┬─────────┬───┘
          │         │
          │         └──────────────────────────────────┐
          ▼                                            ▼
┌───────────────────┐                        ┌───────────────────┐
│ Management API Rtr│ (/management-api/*)    │    API Router     │ (/api/*)
│ (CORS)            │                        │ (CORS)            │
│ (No fallback)     │                        │ (No fallback)     │
└─────────┬─────────┘                        └─────────┬─────────┘
          │                                            │
          ▼                                            ▼
┌───────────────────┐                        ┌───────────────────┐
│  Mgmt API v0 Rtr  │ (/management-api/v0/*) │ Format Sub-router │ (/openai-v1/*)
│ (Management Auth) │                        │ (Format Auth)     │
│ (404: errno JSON) │                        │ (404: OpenAI JSON)│
└───────────────────┘                        └───────────────────┘
```

The server architecture separates low-level connection driving from hierarchical routing:
1. **Server Lifecycle (`server` module + `hserver`)**:
   - The gateway's own `server` module owns the TCP listener (`socket2`: dual-stack IPv4/IPv6, non-blocking, explicit listen backlog) and the accept loop, which retries failed accepts with exponential backoff and hands every accepted connection to one `hserver` connection driver — built once at startup with the HTTP/1 and HTTP/2 configurations over the root router, so serving a connection clones the driver's shared configuration instead of rebuilding it — which drives the low-level Hyper connection lifecycle — HTTP/1 or HTTP/2, selected per connection by protocol auto-detection (the HTTP/2 connection preface for plaintext prior-knowledge clients; ALPN for TLS clients) — directly over the accepted TCP stream, or wrapped in TLS (`rustls` via hserver's `tls` feature) when the `tls` configuration section (§8) is present. TLS applies to the whole listener: the PEM certificate chain and private key are loaded and combined into one shared Rustls server configuration when the configuration loads and recombined whenever it or its material changes (§8), and every accepted connection reads the currently active configuration for its handshake — established connections keep serving while new ones pick up a reload. The handshake offers both `h2` and `http/1.1` through ALPN and performs no client-certificate authentication; inbound access is governed by the gateway's own token authentication either way (§4).
   - Startup builds the shared `Context` (`src/context.rs`) — the `Database` handle plus the two upstream HTTP clients, data plane and control plane (§4) — and passes it to `server::Server::new(context)`, which binds the listening socket and builds the shared connection driver over the root router carrying the context as router state; `Server::run` then serves connections until the process is terminated. Startup also spawns the configuration watcher task (§8), which keeps the live configuration current with its files.
   - `axum::serve` is **strictly unused**. Instead, every accepted connection — after its TLS handshake, when TLS is configured — is delegated directly to Axum's root `Router` as an implementation of `tower::Service`.
2. **Hierarchical Routers (`axum::Router`)**:
   - **Root Router**:
     - Holds the shared `Context` — the `Database` handle plus the two upstream HTTP clients — as its router state and propagates it to sub-routers.
     - Nests `/management-api` and `/api` sub-routers.
     - Maintains its own top-level fallback route returning an empty HTTP 404 response for any unmapped root paths.
   - **Management API Sub-router (`/management-api`)**:
     - Parent router for every management API version.
     - Nests the version sub-routers (`/v0`) and applies the management CORS policy (`management_api.cors`).
     - Defines no route of its own: unmapped paths outside every version are answered with the same empty 404 the root router renders (§2.2).
   - **Management API Version Sub-routers (e.g. `/v0`)**:
     - Scoped to that version's management endpoints (`/providers:list`, `/providers:update`, etc.) and owning that version's payloads and error taxonomy.
     - Wrapped with management authentication middleware (`Authorization: <token>`) and the management body limit.
     - Answers unmapped paths beneath the version prefix with that version's errno-shaped 404 JSON error response (§7).
   - **API Sub-router (`/api`)**:
     - Parent router for all external gateway protocols.
     - Nests isolated format sub-routers (e.g. `/openai-v1`).
     - Defines no route of its own: unmapped paths outside every format are answered with the same empty 404 the root router renders (§2.2).
   - **Format-Specific Sub-routers (e.g. `/openai-v1`)**:
     - Scoped to that protocol's endpoints (`/chat/completions`, `/responses`, `/models`).
     - Wrapped with format-specific authentication middleware (e.g. `Authorization: Bearer <key>`).
     - Answers unmapped paths beneath the format prefix with format-compliant 404 error payloads (e.g. OpenAI JSON error structure: `{"error": {"message": "Invalid URL", "type": "invalid_request_error", "code": null}}`).
3. **Independent 404 Responses**:
   - Each routing domain answers its own unmapped paths: the root router with an empty 404, the management API v0 version sub-router with version 0's errno error JSON (§7), and the `/openai-v1` format sub-router with the OpenAI error JSON. The `/api` and `/management-api` parent routers own no route, so their unmatched paths reach the same empty 404.
   - That empty 404 is one shared handler: the root router declares it, and the two parent routers declare that same handler as their own fallback instead of inheriting it. axum 0.8 discards a nested router's fallback — together with every fallback collected from routers nested below it — when the router being nested declares none of its own (tokio-rs/axum#3138; axum 0.9 fixes the propagation through tokio-rs/axum#3158), so without the parents' declaration the sub-routers' 404s below them would never run. The parent declarations are marked `TODO(axum 0.9)` and disappear with that release; the observable behavior is identical either way, because all three name the same handler.
4. **Method Mismatch on Routed Paths**:
   - A request whose method is not registered on an otherwise-matching routed path never reaches the 404 responses: axum's method routing answers HTTP 405 with an `Allow` header listing the registered methods. Within a management API version sub-router that 405 renders the version's error body (errno 8 `method_not_allowed`); the outward `/openai-v1` format sub-router answers it with an empty body.
   - This 405 is distinct from the 404 responses above, which serve only unmapped paths; the router-level authentication middleware still applies, so an unauthorized request is rejected with 401 before the 405 is produced.

**Fail-fast panics are contractual**: the process installs a panic hook before any other startup operation that prints the panic diagnostic to stderr and terminates the process with exit code 1, so a panic in any spawned task — accepted-connection or request-pipeline — kills the whole gateway by design instead of leaving a partially operational server behind.

---

## 3. Storage & Schema (Turso)

The database runs in-process via `turso` with SQLite-compatible syntax and strict column typing. The schema is versioned in `src/database/schema/`: every version is one `version_x.rs` module defining `const SCHEMA: Schema` — its `version`, its complete DDL (`schema`, hand-written SQL), and the `migration` SQL that transforms the previous version into it — registered in ascending order in the module's `SCHEMAS` table, starting at version 1 with no gaps.

### Database Abstraction & Connection Pooling
- **Connection Pool**: Database access is managed through an in-process connection pool (`deadpool`), maintaining up to `database.max_connections` concurrent `turso::Connection` instances. When every connection is checked out, connection acquisition waits at most `database.pool_timeout` milliseconds before failing with `PoolTimeout`.
- **Connection Busy Timeout**: Every pooled connection and schema-bootstrap connection configures a 5-second busy timeout, so a write waits at most 5 seconds for the database file lock before failing with the underlying busy error.
- **Implicit Prepared Statement Cache**: All queries and executions on both `Database` and `Transaction` pass through `prepare_cached()`. Turso's statement cache is per-connection and persists across pool checkouts, so warm connections reuse their cached preparations; a freshly created pooled connection starts cold.
- **Eager Query Materialization**: `query()` drains every result set to completion and returns owned `Vec<Row>` values before the pooled connection is released. Lazily-consumed cursors are never exposed, so a result can never outlive its connection checkout, interleave with another borrower's statements, or leave unfinished statement state on a recycled connection.
- **Transaction Semantics**: A `Transaction` checks out and exclusively holds a pooled connection for its entire lifetime. Statement execution on the handle requires exclusive access (`execute()`/`query()` take `&mut self`), so statements within one transaction can never run concurrently or interleave. Explicit `commit()` or `rollback()` consumes the transaction and returns the connection to the pool; dropping an uncommitted transaction discards its connection instead of returning it to the pool, so the open engine-level write lock is released immediately rather than parked until the connection's next checkout. Connections that still report an open transaction at recycle time are discarded and replaced by the pool.
- **Error Domain**: The database module defines an `Error` enum using `thiserror` that directly wraps underlying Turso engine errors (`Turso`), alongside pool lifecycle errors (`PoolClosed`, `PoolTimeout`), filesystem I/O (`Io`), and invalid UTF-8 database path errors (`InvalidPath`). Connection pool errors with a backend cause are merged directly into `Turso`.
- **No Global Database Static**: The database is opened via `Database::open(path, max_connections, pool_timeout)` during startup, yielding an owned `Database` instance that startup wraps into the shared `Context` (§2.2) passed to `server::Server::new()` and injected into the Axum router hierarchy as shared state.
- **Schema Versioning & Migrations**:
  - Every modification to the database schema is a new version: a new `version_x.rs` module carrying its hand-written DDL — every statement is parsed by `turso_parser` and applied to a fresh database in tests — together with the migration that upgrades a database of the previous version, and a `SCHEMAS` registration. Version 1's migration is empty: a fresh database receives its DDL directly.
  - On startup, `Database::open()` reads `PRAGMA user_version` and binary-searches `SCHEMAS`: version `0` means fresh — it receives version 1's DDL and then continues the same forward walk, so a fresh database ends at the latest version however many exist; a known version is migrated forward one version at a time, each schema or migration text committed atomically together with its `user_version` advance, so a crash mid-chain leaves the database on its last committed version and the next startup resumes from there; a database already at the latest version starts immediately.
  - An unknown version — negative or newer than the latest — panics immediately, failing fast to prevent execution against a database this build cannot interpret.
  - Migrations run under foreign key enforcement: the bootstrap connection enables `PRAGMA foreign_keys` before the walk, and each migration executes inside its own transaction, where SQLite defines the pragma as a no-op (Turso currently accepting it mid-transaction is a divergence from SQLite that may be aligned at any time). A migration must therefore never rely on disabling enforcement — it must be authored to hold under enforcement statement by statement, ordering table-rebuild steps so referential integrity holds at every intermediate state.
  - Tests enforce the `SCHEMAS` ordering invariant (contiguous from 1), an empty version 1 migration, a per-version test applying each DDL to a fresh database, and — each step on its own fresh database — that version 1's DDL and every later version's predecessor-DDL-plus-migration leave the database at exactly the declared schema, compared through a canonical structural form (object set, column order, constraints, foreign keys, index definitions) rather than DDL text.

### Core Entities
- **Provider**:
  - `id`: Integer primary key; the provider's public identifier. Every management reference to an existing provider — updating, renaming, deleting, discovering models, saving model overrides, re-authorizing — goes through it, and it never changes.
  - `name`: Non-empty, unique, renameable label. Its content — characters, case, length — is the administrator's choice and is never validated beyond non-emptiness; the gateway stores it verbatim. The column carries a `CHECK (name <> '')` constraint, so the empty string is a storage-level violation as well. Renaming is an ordinary `providers:update` request; the id and every model routing stay untouched.
  - `model_prefix`: Non-null string prefixed to model names (e.g. `codex/` or `agy-`); may be the empty string. Stored verbatim with no default: every write states the prefix explicitly.
  - `type`: `openai_response`, `openai_chat_completion`, `gemini_generate_content`, `codex_oauth`, or `antigravity_oauth` — one per upstream product, so the type pins the wire format outright and no second column qualifies it.
  - `base_url`: Upstream base URL for static-key providers (`openai_response`, `openai_chat_completion`, `gemini_generate_content`); `NULL` for OAuth logins, whose endpoints are fixed. Stored verbatim with no default: every static-key write states it explicitly.
  - `credentials`: Plaintext JSON payload (`{accessToken, refreshToken, accountId/projectId}` or `{apiKey}`) stored directly in the database without encryption.
  - `expires_at`: Unix timestamp in microseconds when the OAuth access token expires. Credentials replaced through `providers:update` are stored with no expiry, which the gateway treats as not-fresh: the first inference request then performs one lease-guarded refresh (see `refresh_started_at`) using the stored refresh token. A token response reporting a zero or negative `expires_in` is rejected as malformed (502); the `exp`-claim fallback used when the token response omits `expires_in` rejects a non-future `exp` claim as a malformed token response the same way, and an opaque access token carrying no decodable `exp` claim falls back to the default 86400-second lifetime. A token grant rejected with a terminal failure code fails with 502 whose message names the code and the recovery: a dead refresh token — `invalid_grant` in the RFC 6749 §5.2 string form Google sends, or the `auth.openai.com` object-form codes `token_expired`, `invalid_token`, `refresh_token_expired`, `refresh_token_reused`, `refresh_token_invalidated` (compared case-insensitively, mirroring the CLI) — states the refresh token was rejected and the provider needs re-authorization — only user re-authorization recovers a revoked or expired grant, so re-attempting it is futile; a dead authorization code — `invalid_grant`, `token_expired`, `invalid_token`, or `token_exchange_user_error` — states the code is expired, already used, or invalid and directs a fresh flow completed promptly — authorization codes are single-use and die before the flow's own state does (§7.2). Any other token-endpoint rejection keeps the generic upstream rendering with the foreign status embedded. The management errno remains the upstream-error category (502, §7) in all cases.
  - `refresh_started_at`: Lease timestamp in microseconds to prevent concurrent token refresh races. When the token is expired, a request wins the lease by writing `refresh_started_at = now` inside a transaction conditional on the lease being absent or stale (older than 90 seconds); the winner refreshes the token and clears the lease. Concurrent requests poll up to 100 seconds — deliberately longer than the 90-second staleness window, so a lease abandoned by a crashed refresher is always reacquired rather than waited out — until the token is renewed, then re-read it; when the poll window elapses the request fails with 502. The lease acquire is a read-then-write transaction: when a concurrent commit invalidates its read snapshot, the engine refuses the write upgrade with the stale-snapshot busy error, which is classified as "lease not acquired" and joins the same poll loop, so a burst of simultaneous acquirers on one expired token waits its turn instead of failing. Any refresh failure — the grant or its persistence — clears the lease before the original error is returned, so waiters can acquire it instead of burning their whole wait behind a refresh that already failed; a failed lease clear never replaces that original error. The renewed credentials persist only while the refresher still owns the lease it acquired (`refresh_started_at` still equals the acquired stamp): a credentials replacement or stale-lease reacquisition that lands while the grant is in flight wins the row, the in-flight result — derived from superseded credentials — is discarded without touching the newer state, and the loser re-enters the lease competition.
  - `is_enabled`: Stored serving switch (`0` or `1`, default `1`). Master switch for all models of this provider.
- **Model**:
  - `id`: Integer `AUTOINCREMENT` primary key; the row's identity, never recycled (see *Identity is never recycled* below). Every row-addressed write — `models:update`, `models:delete` — goes through it.
  - `(provider_id, upstream_id)`: the natural key, unique — one upstream identifier appears at most once per provider. It is the address creation uses, because the row does not exist yet.
  - `upstream_id`: Exact model identifier discovered from upstream; used verbatim for upstream requests without automatic mapping.
  - `public_id`: Public routing ID (`id_override ?? (model_prefix + upstream_id)`), maintained at write time and unique across the gateway — two model rows can never route one out-facing ID.
  - `id_override`: Non-NULL explicitly sets the public routing ID directly, bypassing `model_prefix`; NULL computes the ID from `model_prefix + upstream_id`.
  - `is_enabled`: Stored serving switch (`0` or `1`, default `1`), written only by administrators (`models:create`, `models:update`) and never by discovery. The provider's `is_enabled` status acts as the master switch.
  - `discovery_state`: `discovered`, `vanished`, or `manual` — no default; every write declares it explicitly. State machine:
    - `discovered`: listed by its provider's most recent successful discovery. Only rows in this state are ever marked `vanished`.
    - `vanished`: listed by an earlier discovery but missing from the most recent one — **service suspended**: the three-input effective status below is false, so the row 404s on routing, is absent from `GET /api/openai-v1/models`, and is listed by the management surface with `is_effectively_enabled: false`. Exactly two recovery paths: the upstream lists the identifier again — the state flips back to `discovered` and service resumes automatically — or an administrator claims the row through `models:create`, which lands it as `manual`.
    - `manual`: created — or claimed out of a vanished row — through `models:create`. A discovery that lists its identifier flips it to `discovered`; one that does not leaves it untouched: a manual row is never automatically marked `vanished`.
    - Discovery never deletes a row and never writes `id_override`, `is_enabled`, `id`, or `public_id` — **the write-permission invariant: discovery writes only `discovery_state`**. Deletion is administrator-only (`models:delete`); a deleted `discovered` row is rebuilt by the next discovery as a new row with a fresh identity. `models:update` writes only `id_override` and `is_enabled` — it can neither change the state nor restore a vanished row.
  - Effective serving — the one three-input definition shared by routing, the enabled-model listing (§6.5), and the management rendering: `provider.is_enabled && model.is_enabled && discovery_state != 'vanished'`. A disabled provider turns off all its models without exception, and a vanished row's suspended service never depends on its switch being written.

  **Identity is never recycled.** Every integer identity a foreign reference can point at — `providers.id`, `api_keys.id`, and `models.id` — is an `INTEGER PRIMARY KEY AUTOINCREMENT` key: SQLite keeps a per-table high-water mark and never hands a deleted row's number to a later row. `oauth_states.id` carries `AUTOINCREMENT` too, so every row identity follows the one rule even though nothing references it today. The identities that are actually referenced — `providers`, `api_keys`, and `models` — are exactly the ones a recycled number would silently reattach a stale reference to: a management request addressing a deleted row, a foreign reference, a log record naming a row. AUTOINCREMENT's bookkeeping tables (SQLite's `sqlite_sequence`, Turso's `__turso_internal_*` sequence tables) are internal objects, excluded from the structural schema comparison.
- **OAuthState**:
  - Pending OAuth authorization flow (`oauth_states`).
  - `state`: Opaque random URL-safe token identifying the flow; returned to the administrator together with the authorize URL and required again at completion.
  - `type`: Provider type the flow is for (`codex_oauth` or `antigravity_oauth`).
  - `verifier`: PKCE code verifier for `codex_oauth`; empty string for `antigravity_oauth` (which authenticates with a client secret instead of PKCE).
  - `expires_at`: Unix timestamp in microseconds after which the flow is invalid (10 minutes after creation).
  - `oauth_states` rows are provider-agnostic: they are keyed by the opaque `state` token and never attached to a provider, so a state completed after its provider was deleted legitimately recreates the provider.
  - State consumption is a single atomic `DELETE … RETURNING` statement, so exactly one concurrent completion can ever observe a state and a state can never complete twice. A rejected (expired or unknown) state consumes nothing, and expired rows are swept whenever `oauth:start` runs; completion of an unknown, expired, or already-consumed state yields 404.
- **ApiKey**:
  - `id`: Integer primary key; the key's public identifier. Every management reference to an existing key — updating or deleting — goes through it, and it never changes.
  - `name`: Non-empty, non-unique human label. Its content is the administrator's choice and is never validated beyond non-emptiness; the column carries a `CHECK (name <> '')` constraint, so the empty string is a storage-level violation as well.
  - `key`: Plaintext shared secret presented as `Authorization: Bearer <key>`. Unique across the gateway — a value another row already holds is rejected with 409 — and stored directly in the database without encryption. The column carries a `CHECK (key <> '')` constraint.
  - `is_enabled`: Stored serving switch (`0` or `1`, default `1`). Only enabled keys authorize traffic; disabling a key stops it on the next request.
  - `created_at`: Creation timestamp in UTC microseconds, stamped server-side at insertion.

  Every stored timestamp in the database — `expires_at`, `refresh_started_at`, `created_at` — renders UTC microseconds. Upstream wire units are untouched: a token endpoint's `expires_in` arrives in seconds and is scaled to microseconds on write, and outward protocol `created` fields still render seconds.

---

## 4. Security & Authentication

1. **Inbound Authentication**:
   - Authentication is format-specific and enforced at the router layer via Tower middleware before requests reach endpoint handlers:
     - **Management Version Sub-router (`/management-api/v0/*`)**: Uses management authentication middleware applied to the version sub-router. Requires `Authorization: <token>` without any `Bearer` prefix, matching `management_api.token` in configuration.
     - **Outward API Sub-router (`/api`)**: Performs no authentication itself and owns no route; it nests the format sub-routers and answers its unmatched paths with the same empty 404 the root router renders (§2.2). Each format-specific sub-router enforces authentication and protocol-compliant header parsing independently — the `/openai-v1` sub-router validates client requests against the enabled gateway API keys stored in the database and managed through the management API (§7.1), via `Authorization: Bearer <key>` conforming to the OpenAI specification. The `Bearer` scheme is compared case-insensitively (RFC 9110 §11.1) while the key itself is compared byte-exactly; malformed header forms (missing space, tab separator, glued scheme, leading whitespace, trailing content) are rejected.
   - All token comparisons use constant-time byte comparison. Mismatch yields HTTP 401. An empty or fully disabled key table authorizes nothing: a fresh database serves no outward traffic until the first key is created through the management API.
   - Error response bodies are router-domain specific:
     - Root router: plain HTTP 404 with an empty body.
     - Management API version sub-router (`/management-api/v0/*`): `{"errno": <numeric errno>, "message": <human-readable message>}`; the closed errno taxonomy and each errno's pinned HTTP status are defined in §7. An unmapped path outside every management API version reaches no version's 404 response and is answered with the shared empty 404 (§2.2).
     - Outward API (`/openai-v1` format 404): OpenAI JSON error structure `{"error": {"message": ..., "type": ..., "param": null, "code": ...}}`; the 404 response carries `type: "invalid_request_error"`, `code: null` and `message: "Invalid URL (<METHOD> <path>)"` with HTTP 404.
2. **Plaintext Credential Storage**:
   - Upstream credentials and tokens are stored directly in plaintext JSON within the database, and gateway API keys are stored directly in plaintext alongside them. No encryption or decryption is performed.
3. **Network Invariants**:
   - Inbound endpoints do not use sessions or cookies. Cross-Origin Resource Sharing (CORS) is configurable independently on the outward API sub-router (`/api`) and the administrative management API sub-router (`/management-api`) via `cors` (`allowed_origin`, `allow_private_network`) in configuration. Preflight `OPTIONS` requests are handled and matching CORS response headers are attached, including Private Network Access headers when enabled.
   - Accepted inbound TCP connections enable `TCP_NODELAY` (best-effort: a failure is logged and the connection is still served), so small streaming frames — SSE events in particular — are written immediately instead of stalling against Nagle's algorithm and the peer's delayed ACK.
   - Inbound connections speak HTTP/1 or HTTP/2, selected per connection by hyper-util's protocol auto-detection; TLS connections offer both `h2` and `http/1.1` through ALPN.
   - HTTP/1 connections that do not finish sending request headers within 30 seconds are closed; the deadline engages once protocol detection has selected HTTP/1. HTTP/2 connections are probed with keep-alive PING frames: a connection idle for 30 seconds is pinged, and a ping that goes unacknowledged for 20 seconds closes the connection. A PING must be answered by the peer's HTTP/2 stack — the kernel cannot answer on its behalf, unlike TCP keep-alive — so live-but-idle clients survive while dead or silent HTTP/2 peers are reaped.
   - Connections may park indefinitely between acceptance and their first request headers: neither hyper-util's protocol auto-detection nor the Rustls TLS handshake exposes a deadline, and no first-request deadline is imposed deliberately. A first-request deadline would burden legitimately slow clients while failing to bound resource exhaustion — an attacker holding sockets via slow request bodies or slow keep-alive traffic makes the same investment as one holding sockets silently — so the socket-exhaustion threat is answered, if ever, by connection-count limiting at the accept edge, not by opening-phase timeouts.
   - Outbound requests to upstream providers connect directly without proxying.
4. **Resource Bounds** (value — where enforced — what it bounds):
   - Upstream connect timeout, 10 s — both upstream HTTP clients — TCP connection establishment to an upstream provider.
   - Upstream read idle timeout, 300 s — data-plane upstream HTTP client — silence gap between consecutive reads of an upstream body (reset after every successful read, so it never truncates a slow-but-flowing response; the data plane sets no total request timeout, so a long generation is never cut off).
   - Control-plane total timeout, 30 s — control-plane upstream HTTP client — total wall-clock budget of one short control-plane request (OAuth token exchange and refresh, model discovery, project onboarding §5), so a stalled request fails below the 90-second refresh-lease staleness window (§3) instead of holding its lease or the requesting client for minutes.
   - Inbound HTTP/1 header-read deadline, 30 s — HTTP/1 connection builder — time a client may take to finish sending request headers before the connection is closed; it engages once protocol detection selects HTTP/1 (hyper's effective default is pinned explicitly, as the default is declared unstable).
   - Inbound HTTP/2 keep-alive PING interval, 30 s — HTTP/2 connection builder — idle time before an HTTP/2 connection is probed with a PING frame.
   - Inbound HTTP/2 keep-alive acknowledgment deadline, 20 s — HTTP/2 connection builder — time a peer has to answer a keep-alive PING before the connection is closed.
   - Inbound HTTP/2 concurrent streams per connection, 200 — HTTP/2 connection builder — concurrent request streams one connection may hold (hyper's effective default is pinned explicitly, as the default is declared unstable).
   - Inbound HTTP/2 header list size, 16 KiB — HTTP/2 connection builder — HPACK-decompressed header bytes one request may carry; larger header lists are rejected, bounding HPACK-bomb memory (hyper's default is ~16 MB and declared unstable).
   - TCP listen backlog, 32 — TCP listener setup — pending, not-yet-accepted connections the kernel queues before refusing new ones.
   - Request body caps, 64 MiB outward and 2 MiB management — axum body-limit layer on each sub-router — larger request bodies are rejected with HTTP 413.
   - SSE frame cap, 16 MiB — SSE frame parser — a single upstream SSE frame; a larger frame aborts the stream as malformed.
   - Channel buffers, 64 — between the upstream pump and the event renderer, and between the SSE renderer and the response body — backpressure between pipeline stages.
   - Upstream error-body reads, 4 KiB with 400-character excerpts — every upstream failure the gateway renders (gateway error mirroring §6.7, OAuth token endpoint rejections §5.1/§5.2, model discovery rejections §7.1) — memory held to carry an upstream failure message; success bodies are legitimate JSON of unbounded size and are read in full.
   - Accept-failure backoff, 10 ms doubling to a 1 s cap — TCP accept loop — retry delay after a failed accept attempt.
   - OAuth onboarding budget, 60 s — `onboardUser` long-running operation loop (§5.2) — total wall-clock time for project-ID resolution.

---

## 5. Provider Upstream Specifics

### 5.1 Codex OAuth (`codex_oauth`)
- **OAuth Constants**: Client ID `app_EMoamEEZ73f0CkXaXp7hrann`, PKCE S256, loopback redirect `http://localhost:1455/auth/callback`.
  - PKCE generation: `code_verifier` is Base64URL (unpadded) of 64 random bytes; `code_challenge` is Base64URL (unpadded) of SHA-256 over the ASCII verifier bytes.
  - Authorize URL: `https://auth.openai.com/oauth/authorize?response_type=code&client_id=<client_id>&redirect_uri=http://localhost:1455/auth/callback&scope=openid%20profile%20email%20offline_access%20api.connectors.read%20api.connectors.invoke&code_challenge=<challenge>&code_challenge_method=S256&id_token_add_organizations=true&codex_cli_simplified_flow=true&state=<state>&originator=codex_cli_rs`. The scopes and the two flags mirror the Codex CLI's authorize URL verbatim, so the issued code is redeemable by the same exchange.
  - Token exchange: `POST https://auth.openai.com/oauth/token` with `application/x-www-form-urlencoded` body `grant_type=authorization_code&code=<code>&redirect_uri=<redirect>&client_id=<client_id>&code_verifier=<verifier>`; response is JSON `{id_token, access_token, refresh_token, expires_in?}`.
  - Token refresh: `POST https://auth.openai.com/oauth/token` with JSON body `{"client_id": <client_id>, "grant_type": "refresh_token", "refresh_token": <refresh_token>}`; response is JSON `{id_token?, access_token?, refresh_token?}`.
  - The ChatGPT account ID is extracted from the `chatgpt_account_id` claim inside the `https://api.openai.com/auth` claim object of the `id_token` JWT payload.
- **Upstream URL**: `https://chatgpt.com/backend-api/codex/responses`
- **Mandatory Request Invariants**:
  1. `store: false` must always be present.
  2. `stream: true` is strictly required by the upstream Codex endpoint. When a gateway client requests non-streaming mode (`"stream": false`), the gateway **must** request streaming upstream, buffer and aggregate the SSE chunks in memory, and return a single aggregated non-streaming JSON response.
  3. `max_output_tokens` must never be rendered: the upstream Codex endpoint rejects the parameter as unsupported, and the model's own output bound applies instead of the client-requested cap (the cap is consumed by translation and dropped).
  4. Required headers:
     ```text
     Authorization: Bearer <accessToken>
     chatgpt-account-id: <accountId>
     Originator: codex_cli_rs
     User-Agent: codex_cli_rs/0.147.0
     Session-Id: <uuid>
     Accept: text/event-stream
     ```
     The `Session-Id` is stamped fresh (a new UUID) for every upstream request: the gateway is a stateless proxy with no session concept. This deliberately diverges from the mirrored Codex CLI that pins §5's constants, which reuses one session id for its whole session.
- **Model Discovery**: `GET https://chatgpt.com/backend-api/codex/models?client_version=<client_version>` with `Authorization: Bearer <accessToken>` and `chatgpt-account-id: <accountId>` headers, where `<client_version>` is the CLI version pinned in the `User-Agent` (`0.147.0`). The response payload must contain an `items` or `models` array whose entries carry a `slug` string; each slug is one `upstream_id`. Any other shape fails discovery.

### 5.2 Google Antigravity OAuth (`antigravity_oauth`)
- **OAuth Constants**: Installed-app client ID `1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com`, client secret `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`, loopback redirect `http://localhost:51121/oauth-callback`. No PKCE; the client secret authenticates the exchange.
  - Authorize URL: `https://accounts.google.com/o/oauth2/v2/auth?access_type=offline&client_id=<client_id>&prompt=consent&redirect_uri=http://localhost:51121/oauth-callback&response_type=code&scope=<scopes %20-joined>&state=<state>`.
  - Scopes: `https://www.googleapis.com/auth/cloud-platform`, `https://www.googleapis.com/auth/userinfo.email`, `https://www.googleapis.com/auth/userinfo.profile`, `https://www.googleapis.com/auth/cclog`, `https://www.googleapis.com/auth/experimentsandconfigs`. Both authorize URLs percent-encode the space-joined scope list as `%20` (RFC 3986 forbids raw spaces in a query).
  - Token exchange: `POST https://oauth2.googleapis.com/token` with form body `code=<code>&client_id=<client_id>&client_secret=<secret>&redirect_uri=<redirect>&grant_type=authorization_code`; response is JSON `{access_token, refresh_token, expires_in, ...}`.
  - Token refresh: same endpoint with form body `refresh_token=<refresh_token>&client_id=<client_id>&client_secret=<secret>&grant_type=refresh_token`.
- **Project ID Resolution**: The Antigravity project ID (`projectId` credential) is resolved once during OAuth completion:
  1. `POST https://daily-cloudcode-pa.googleapis.com/v1internal:loadCodeAssist` with JSON body `{"metadata": {"ideType": "ANTIGRAVITY", "platform": "PLATFORM_UNSPECIFIED", "pluginType": "GEMINI"}}`. If the response carries `cloudaicompanionProject` — the bare project-id string — that is the project ID.
  2. Otherwise `POST https://daily-cloudcode-pa.googleapis.com/v1internal:onboardUser` with JSON body `{"tierId": <tier>, "metadata": {...}}` where `<tier>` is `paidTier.id` when present, else `currentTier.id`, else `free-tier`. The long-running operation is polled by repeating the request until `done` is true, within a 60-second total wall-clock budget checked before every attempt (each attempt is bounded by the control-plane client's total timeout, but a fixed attempt count alone bounds nothing); an exhausted budget fails the resolution. A completed operation carrying a terminal `error` payload (`google.longrunning` failure shape `{code, message, status}`) fails the resolution with that status and message. The project ID is `response.cloudaicompanionProject`, which the operation result wraps as an `{id}` object (a bare string is also accepted defensively); a missing or empty id fails the resolution.
  - Failure to resolve a project ID fails the OAuth completion.
- **Upstream URL**: `https://daily-cloudcode-pa.googleapis.com/v1internal`
  - *Note*: Personal/consumer accounts must use `daily-cloudcode-pa.googleapis.com`; `cloudcode-pa.googleapis.com` returns `429 RESOURCE_EXHAUSTED`.
- **Upstream Actions**:
  - Non-streaming: `:generateContent`
  - Streaming: `:streamGenerateContent?alt=sse`
- **Headers**: `Authorization: Bearer <accessToken>`, `Content-Type: application/json`, `User-Agent: antigravity`. Every Antigravity call — project resolution (`loadCodeAssist`, `onboardUser`), model discovery (`fetchAvailableModels`), and inference — sends this header set.
- **Envelope**:
  ```json
  {
    "project": "<projectId>",
    "model": "<upstream_id>",
    "request": <GenerateContentRequest>,
    "requestType": "agent",
    "userAgent": "antigravity",
    "requestId": "agent-<uuid>"
  }
  ```
- **Upstream Invariants**:
  - Every Antigravity response — `:generateContent` JSON bodies and `alt=sse` SSE frames alike — wraps the GenerateContentResponse in a `response` envelope beside `traceId` and `metadata`; the gateway unwraps the envelope when present and equally accepts bare GenerateContentResponse payloads (as sent by Gemini-compatible upstreams).
  - `thoughtSignature` blobs on function-call parts must be preserved and reattached across turns; text-part signatures are not carried — the outward OpenAI surfaces have no wire slot for them — and are dropped at parse time.
  - `functionCall.id` values are preserved across turns like `thoughtSignature`: a populated upstream call id seeds the IR `call_id` (an id-less call mints `call_<uuid>`, the same id the outward OpenAI surfaces expose as the tool-call id), and the replayed history emits it as the `FunctionCall.id` of the call and the matching `FunctionResponse.id` of its tool result, so parallel calls to one function name resolve by id rather than by name.
  - Image parts carrying a public `http`/`https` URL render to Gemini upstreams — `antigravity_oauth` here and `gemini_generate_content` (§5.3) alike — as `fileData.fileUri`: the current Gemini API accepts any publicly accessible web URL there and infers the MIME type upstream. The gateway neither fetches nor inlines images; delivering the URL is the upstream's job, and a URL it cannot fetch mirrors its upstream error per §6.7.
  - No automatic model mapping or tiered rewriting is performed; upstream calls pass the exact discovered `upstream_id`.
- **Model Discovery**: `POST https://daily-cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels` with JSON body `{}`; the response payload's `models` object keys are the `upstream_id` values.

### 5.3 Static-Key Providers (`openai_response`, `openai_chat_completion`, `gemini_generate_content`)
- User-specified `baseUrl` and `apiKey`; the provider's `type` is its wire format outright.
- Upstream calls target `baseUrl` with `Authorization: Bearer <apiKey>` (`openai_response`, `openai_chat_completion`) or `x-goog-api-key: <apiKey>` (`gemini_generate_content`).
  - `openai_chat_completion`: `POST <baseUrl>/chat/completions`.
  - `openai_response`: `POST <baseUrl>/responses`.
  - `gemini_generate_content`: `POST <baseUrl>/models/<upstream_id>:generateContent` and `:streamGenerateContent?alt=sse`; the request body is the bare GenerateContentRequest (no Antigravity envelope).
- **Model Discovery**:
  - `openai_response` / `openai_chat_completion`: `GET <baseUrl>/models`; response `{data: [{id, ...}]}`.
  - `gemini_generate_content`: `GET <baseUrl>/models`; response `{models: [{name, ...}]}` where `name` is either `"models/<id>"` or bare `<id>`.

---

## 6. Translation Architecture (IR)

All incoming and outgoing payloads map through an internal Intermediate Representation (IR):

```text
[Client Request] ──> [Parse to IR] ──> [Render Upstream Payload] ──> [Upstream]
                                                                        │
[Client Response] <── [Render Client Format] <── [Parse IR Events] <─────┘
```

### 6.1 IR Request

- `model`: resolved upstream model name (see §6.5); rendered verbatim into upstream payloads.
- `instructions`: concatenated system instructions.
- `messages`: ordered list of messages.
  - `role`: `user`, `assistant`, or `tool`.
  - `parts`: `Text {text}`, `Image {Inline {mime_type, data_base64} | Url (url)}`, `ToolCall {call_id, name, arguments, thought_signature?}`, `ToolOutput {call_id, content}`.
- `tools`: function declarations (`name`, `description?`, `parameters?` JSON Schema).
- `tool_choice`: `Auto | None | Required | Function {name}`.
- Sampling: `temperature`, `top_p`, `max_output_tokens` (all optional).
- `response_format`: `Text | Json | JsonSchema {name, schema}`.
- `reasoning_effort`: the client's own reasoning-effort string, opaque to the gateway — only its type is constrained (§6.4) — or absent.

### 6.2 IR Events

- `MessageStart {id, model}` — identifies the completion; emitted exactly once, first.
- `TextDelta (delta)` — incremental assistant text.
- `ReasoningDelta (delta)` — incremental reasoning/summary text.
- `ToolCallDelta {index, id?, name?, arguments_delta}` — incremental tool call assembly; `index` is stable per call within the response.
- `ToolCallDone {index, call_id, name, arguments, thought_signature?}` — complete tool call.
- `Finish (reason)` — `Stop | Length | ToolCall | ContentFilter`.
- `Usage {input_tokens, output_tokens, cached_tokens?, cache_write_tokens?, reasoning_tokens?}` — token accounting. The three breakdown values are `None` when the upstream reports no token-detail objects (every Chat Completions upstream, and any Responses upstream that reports totals only).
- `Done` — terminal event; always the last event of every event stream, regardless of upstream shape. Parsers never emit it; the gateway appends it when the upstream response is fully consumed.

Every upstream response — SSE stream or JSON document — is parsed into an IR event stream; every client response — streamed or aggregated — is rendered from IR events. Upstream payloads are third-party data modelled member by member: members the gateway does not read are ignored, while a member it reads whose JSON kind contradicts the protocol — a non-string text or a non-object tool-call arguments — makes the payload malformed and fails the request per §6.7 rather than being silently skipped. The parser is chosen by the upstream streaming mode the gateway requested — always streaming for `codex_oauth` (§5.1), otherwise the client's requested mode (§6.3) — never by response headers: the Codex upstream serves its 200 SSE streams without a `Content-Type` header. Non-streaming client responses are aggregated in memory from the same event stream (Text/Reasoning/ToolCall deltas → accumulated content, ToolCallDone → final calls, Finish, Usage; `MessageStart` contributes nothing — outward responses carry renderer-minted identifiers). When a tool-call done event carries no assembled arguments, the accumulated streamed deltas are used; a repeated ToolCallDone for an already-completed call index is ignored. For streamed Chat Completions, a tool-call done for a delta-opened call renders to the client exactly the argument bytes its live deltas did not already deliver — none when they already delivered all of them — a done for a never-opened index renders the complete call as one `tool_calls` delta chunk, and a divergent done whose arguments are not a superstring of the streamed bytes — or whose streamed byte count falls mid-character in the done's arguments — silently skips its residual argument frame.

Upstream refusal text is delivered as assistant message text (`TextDelta`), so it reaches clients through the ordinary text channel: Chat Completions upstreams map `delta.refusal` and `message.refusal` (content text before refusal text when one delta carries both), and Responses upstreams map `response.refusal.delta` events and `refusal` message content parts exactly like `output_text` parts.

### 6.3 Format Matrix

| Outward Endpoint | Client Format | Upstream Formats |
| :--- | :--- | :--- |
| `POST /api/openai-v1/chat/completions` | OpenAI Chat Completions | all three |
| `POST /api/openai-v1/responses` | OpenAI Responses | all three |
| `GET /api/openai-v1/models` | OpenAI List Models | — |

| Provider Type | Upstream Format | Upstream Streaming |
| :--- | :--- | :--- |
| `codex_oauth` | OpenAI Responses | always streaming upstream (invariant §5.1); non-streaming clients get an aggregated response |
| `antigravity_oauth` | Gemini GenerateContent (envelope §5.2) | matches the client's requested mode |
| `openai_response` | OpenAI Responses | matches the client's requested mode |
| `openai_chat_completion` | OpenAI Chat Completions | matches the client's requested mode |
| `gemini_generate_content` | Gemini GenerateContent (bare request §5.3) | matches the client's requested mode |

- Streaming clients receive live translated SSE chunks (`data: {json}\n\n`). Chat Completions streams end with a `data: [DONE]` sentinel; Responses streams end after the terminal `response.completed` or `response.incomplete` event. Every Responses SSE payload embeds its event name as a `"type"` field and a monotonically increasing `sequence_number`, so strict clients that discriminate on the JSON payload work without reading the `event:` line.
- Streaming Chat Completions deliver usage in a single terminal usage chunk immediately before `data: [DONE]`; when an upstream reports usage repeatedly, the last observation wins and renders once. Delta chunks carry no `usage` field at all — usage appears only in the final chunk.
- Responses `response.output_text.delta` and `response.output_text.done` events and `output_text` content parts always carry `logprobs: []` — empty when no logprobs are produced — because the official openai-python client requires the field.
- Every Responses usage object the gateway emits — in the `response.completed`/`response.incomplete` terminal events and in the aggregated non-streaming body — carries exactly `{input_tokens, input_tokens_details: {cached_tokens, cache_write_tokens}, output_tokens, output_tokens_details: {reasoning_tokens}, total_tokens}`: openai-python's `ResponseUsage` types all five fields, and every detail-object integer, as required without defaults, so a breakdown the upstream did not report renders zero-filled (`0`) while upstream-reported values ride through; `total_tokens` is the sum of `input_tokens` and `output_tokens`. The outward Chat Completions usage shape is unchanged (`prompt_tokens`/`completion_tokens`/`total_tokens`).
- Every Responses response object the gateway emits — the `response` field embedded in streaming events, the terminal `response.failed` frame, and the aggregated non-streaming body — carries `parallel_tool_calls: false`, `tool_choice: "auto"`, and `tools: []`: the official Response shape types these fields without defaults, and the statics truthfully describe the gateway (no parallel tool-call control, the model decides when to call tools, no tool definitions are echoed back).
- `Finish(Length)` and `Finish(ContentFilter)` render as `response.incomplete` carrying `incomplete_details.reason` (`max_output_tokens` / `content_filter`), both in the streaming terminal event and in the aggregated non-streaming response's `status`/`incomplete_details`; all other finishes render `response.completed` / `status: "completed"`.
- A turn whose upstream delivered function calls finishes with the canonical reason `ToolCall` — mixed text+tool-call turns included, matching official Chat Completions semantics: the Chat Completions upstream maps its native `finish_reason` (`tool_calls`, `function_call`); the Responses upstream reports `ToolCall` for a turn that delivered function calls under any status other than `incomplete` (the Responses status vocabulary has no tool-call value), including calls recovered from a terminal output array; the Gemini upstream reports `ToolCall` when the turn delivered `functionCall` parts under any finish reason other than `MAX_TOKENS` or a filter reason; a `finishReason` of `MALFORMED_FUNCTION_CALL`, however, is a failed generation — the turn aborts as an upstream failure under §6.7 instead of rendering a clean finish. Truncation maps `MAX_TOKENS` to the length finish, and the flagged-content finishes — `LANGUAGE` (unsupported language), `SAFETY`, `RECITATION`, `BLOCKLIST`, `PROHIBITED_CONTENT`, and `SPII` — map to the canonical content-filter finish; unknown finish reasons degrade to a clean `Stop` finish.
- A Gemini prompt-level block — `promptFeedback.blockReason` present with no finishing candidate; Gemini sends `promptFeedback` only when no candidate was generated — yields the canonical content-filter finish (`finish_reason: "content_filter"` / `response.incomplete` with `incomplete_details.reason: "content_filter"`), and a candidate `finishReason` wins when both are present, so a finishing candidate is never overridden by the feedback.
- Malformed upstream SSE/JSON payloads abort the stream; clients that are already streaming receive a terminal `data: {"error": {...}}` frame (Chat Completions) or a `response.failed` event (Responses) with the translated error. This applies to malformed trailing frames flushed at end-of-stream as well: they abort the stream with an error frame instead of terminating cleanly. Upstream failures delivered inside a success transport abort the same way: an OpenAI-family stream frame or a 2xx JSON body carrying a non-null `error` value aborts with a terminal error frame / 502 instead of parsing as an empty success; a top-level `"error": null` is the normal success shape of the non-streaming Response object and parses normally — only a non-null `error` value aborts.
- The Responses upstream parser recovers message text, reasoning text, and function calls exactly once. Message and reasoning text recovery treats streamed and terminal items as fallback sources consulted only when no live event delivered the kind — live deltas exhaust every fallback source for that kind. A streamed `output_item.done` function_call item always completes its call, and only the terminal array's call copies are gated on live delivery. A static-key upstream that streams complete `output_item.done` items or only a terminal `response.completed`/`response.incomplete` event without live events therefore still delivers its `output`-array text and function calls to the client exactly once. Message and reasoning fallback recovery is keyed by the item's output index: a streamed `output_item.done` item and the terminal array's same-index copy are delivered exactly once together, and distinct indices each deliver exactly once — the dedup rests on the conformant identity between a streamed item's `output_index` and its terminal-array position.
- On a clean upstream end-of-stream the gateway flushes buffered parser state (e.g. pending Chat Completions tool calls) into the event stream before the terminal `Done` event; an aborted stream skips the flush. A clean end-of-stream that carried no finish reason (Chat Completions) or no terminal event (Responses/Gemini) degrades to a `Stop` finish, so streamed and aggregated renderings of the same stream agree.
- Upstream deadlines: two upstream HTTP clients share a 10-second connect timeout. The data-plane client enforces a 300-second read timeout that resets after every successful read of the upstream body and sets no total request timeout: a stalled upstream therefore aborts the request (mapped to 502 per §6.7) instead of hanging it indefinitely, while a slow-but-flowing response is never truncated. The control-plane client (token exchange and refresh, model discovery, project onboarding) bounds each request at 30 seconds total, so a stalled control-plane request fails below the 90-second refresh-lease staleness window (§3).

### 6.4 Reasoning Effort

> **Reasoning Effort.** The value is opaque to the gateway: it is read as a string from the client's own parameter (Chat Completions `reasoning_effort`, Responses `reasoning.effort`) and written verbatim into the upstream's own parameter (Responses `reasoning.effort`, Chat Completions `reasoning_effort`, Gemini `generationConfig.thinkingConfig.thinkingLevel`). The gateway constrains only the type — a non-string value is a 400 malformed request — and otherwise defines, validates, enumerates, translates, clamps, and defaults nothing: there is no value list of any kind, and every value is forwarded as-is, including values the gateway does not recognize, values the target does not support, and the empty string. When the target rejects a value, the failure happens upstream and mirrors per §6.7; the upstream is the sole authority on what is supported, and the gateway keeps no per-model data. When the client omits the field, no upstream parameter is rendered and the upstream's own default applies — that is not "no thinking". Token budgets are never rendered (the Gemini 2.5-era `thinkingConfig.thinkingBudget` and its kind are never sent): the gateway renders only `thinkingLevel`, so reasoning control is supported only on Gemini 3 generation and later models. Consequently `models` carries no effort column, `models:list` and `GET /api/openai-v1/models` expose no effort, and logs record no requested effort. Reasoning content (reasoning deltas, thoughtSignature) is a separate concern, unaffected (§6.2/§6.6/§5.2).

### 6.5 Model Routing

- Client-visible model IDs are resolved against `models.public_id` (the public routing ID). Resolution requires the model's three-input effective status (§3) to be on; a `vanished` row therefore falls into the pre-existing outcome with **no special-cased path** — its false effective status routes it into the same HTTP 404 OpenAI `model_not_found` error as an unknown or disabled model, with the same classification and message. Unknown or disabled models yield that 404 as well.
- The IR `model` is always the `upstream_id` of the resolved row; public IDs never reach an upstream.
- `GET /api/openai-v1/models` lists exactly the rows the three-input definition (§3) marks effective — the same rule as routing, so `vanished` rows are excluded by that rule alone — as `{"object": "list", "data": [{"id": <public ID>, "object": "model", "created": 0, "owned_by": <provider name>}]}`; `created` is a fixed `0` because the gateway does not track model creation times. The management `models:list` instead lists every row, `vanished` included, each carrying its own `is_effectively_enabled`.

### 6.6 Supported Request Fields

Requests are parsed strictly: unknown fields and known-but-unsupported fields are rejected with HTTP 400 `invalid_request_error` naming the offending field. The carve-outs accept and ignore extra fields. For history replay: Responses `message` input items accept and ignore `id`, `phase`, and `status` (codex replays its parsed output items verbatim each turn, and the gateway's own rendered assistant `message` items carry `id` and `status`, so replaying them verbatim must parse); Responses `function_call` and `function_call_output` input items accept and ignore `id` and `status` (the gateway's own rendered `function_call` items carry `id` and `status`, so replaying them verbatim must parse); Responses `output_text` content parts accept and ignore `annotations` and `logprobs` (the gateway's own rendered `output_text` parts carry both, so replaying them verbatim must parse); and Chat Completions assistant messages accept and ignore `reasoning` and `reasoning_content`. Beyond history replay, the `strict` flag is accepted and ignored on Responses function tools, on Responses `text.format` `json_schema`, and on Chat Completions function tools and `json_schema`. Beyond history replay, the Responses request envelope accepts and ignores `parallel_tool_calls`, `include`, `prompt_cache_key`, `stream_options`, `service_tier`, `client_metadata`, and `access_programs`, and `text` accepts and ignores `verbosity` (typed clients — the codex CLI foremost — serialize these stateless-idempotent controls on every request, and a stateless gateway cannot honor any of them downstream, so rejecting them would 400 the client's first request before the item-level carve-outs are ever reached; they never affect routing or the upstream render). Image parts accept and ignore `detail` (typed SDK clients send it on every image part, and the gateway cannot honor it downstream) — Chat Completions `image_url` parts and Responses `input_image` parts. Chat Completions validates assistant `tool_calls[].function.arguments` as a JSON object at parse time, in parity with Responses `function_call` items. Supported fields:

- **Chat Completions**: `model`, `messages`, `tools` (function tools only), `tool_choice`, `temperature`, `top_p`, `max_tokens` / `max_completion_tokens` (both set is rejected), `response_format` (`text`, `json_object`, `json_schema`), `reasoning_effort`, `stream`, `stream_options` (`{include_usage}` only).
- **Responses**: `model`, `input` (string or item array), `instructions`, `tools` (function tools only), `tool_choice`, `temperature`, `top_p`, `max_output_tokens`, `reasoning` (`{effort}` is constrained by its string type alone, §6.4; `summary` is validated — `"auto"` accepted, other values rejected), `stream`, `store` (must be absent or `false`), `text.format` (`text`, `json_object`, `json_schema`); the envelope controls carved out above are additional accepted-and-ignored fields.
- **Message roles**: Chat Completions `system` / `developer` / `user` / `assistant` / `tool`; Responses input items `message`, `function_call`, `function_call_output`, and `reasoning` (reasoning items are accepted and ignored — they carry no replayable state in a stateless gateway).
- System/developer messages concatenate in encounter order into IR `instructions`.
- Responses system/developer message content parts must be text; an `input_image` part is rejected with HTTP 400 naming the field `content` (instructions have no image channel).
- Responses `function_call_output` items accept `output` as a plain string or as an array of content parts (the official wire shapes); text parts flatten into the tool output with the same blank-line separator the Chat Completions text-parts shapes use (the array form flattens to exactly what the equivalent plain-string form carries), and `input_image`/`input_file` parts in the array are rejected with HTTP 400 naming the field `output` (the IR tool output has no media channel).
- Chat Completions `system`, `developer`, and `tool` message `content` may be a plain string or an array of `{"type": "text", "text": ...}` parts (the official API shape); the parts flatten with the same blank-line separator the string forms use — system/developer into IR `instructions`, tool into the tool output — and any other part type in such an array is rejected with HTTP 400 naming the field `content`. Assistant `content` shares the same string-or-text-parts shape and may also be absent.
- Chat Completions reasoning key policy: upstream parsing accepts both `reasoning` (precedence when both are present) and `reasoning_content`; the gateway emits `reasoning_content` in streaming deltas and in aggregated messages.
- **Thought signatures**: `thought_signature` (optional) on Chat Completions `tool_calls[]` entries and Responses `function_call` items is a gateway extension that is accepted inbound and emitted outbound on the matching output positions; it round-trips Gemini thought signatures per §5.2.
- Usage is always included in aggregated Chat Completions responses and in Responses usage-bearing events; streaming Chat Completions responses emit the usage chunk only when `stream_options.include_usage` is set, and that chunk is the final chunk immediately before `data: [DONE]` — exactly once, carrying the last upstream-reported usage when an upstream streams usage repeatedly.

### 6.7 Error Mapping

| Situation | HTTP Status | Outward Body |
| :--- | :--- | :--- |
| Malformed / unsupported request | 400 | OpenAI error, `type: "invalid_request_error"`, `code: null` |
| Invalid API key | 401 | OpenAI error, `type: "invalid_request_error"`, `code: "invalid_api_key"` |
| Unknown or disabled model | 404 | OpenAI error, `type: "invalid_request_error"`, `param: "model"`, `code: "model_not_found"` |
| Upstream returned a non-2xx status | upstream status | OpenAI error, `type: "api_error"`, `code: "upstream_error"`; the response status mirrors the upstream status, and the message carries the upstream body excerpt |
| Upstream transport failure / malformed upstream payload — a stalled or broken transport, a Gemini `MALFORMED_FUNCTION_CALL` finish, or an OpenAI-family 2xx body embedding a non-null `error` value | 502 | OpenAI error, `type: "api_error"`, `code: "upstream_error"` |
| Gateway failure | 500 | OpenAI error object, `type: "api_error"`, `code: "internal"` |

Error payloads for the `/responses` endpoint use the same OpenAI error JSON structure.

---

## 7. Management API (`/management-api/v0/*`)

The management API is versioned. Every version owns its routes, wire payloads, error taxonomy, and bindings, while the route contract, the `route!` declaration macro, and the bindings generator are shared by every version; the management API router mounts each version under its own prefix — currently `/management-api/v0`. Version 0's routes require authorization via `Authorization: <token>` (without any `Bearer` prefix) and are served by the version sub-router, which owns the version's errno-shaped 404 response; an unmapped path outside every version reaches no version's 404 response and is answered with the shared empty 404 (§2.2).

Every route declares its method, path, request payload type, and response payload type exactly once, next to its handler. Both payload types are concrete named types: a route that carries no request body declares an empty request type, and an inline-only type (such as `()`) is never a payload type. The bindings' route table (§7.3) is generated from these declarations.

Responses are JSON. Success responses use HTTP 200; errors use the structure from §4: `{"errno": <errno>, "message": <human-readable message>}`. Each version defines its own error taxonomy; the `errno` field is a numeric code from version 0's closed taxonomy, and each errno pins exactly one HTTP status — the two can never drift apart:

| errno | Category | HTTP Status |
| :--- | :--- | :--- |
| 1 | `invalid_request` — the request is malformed or unsupported | 400 |
| 2 | `unauthorized` — missing or invalid management token | 401 |
| 3 | `not_found` — unknown resource or path | 404 |
| 4 | `conflict` — the request collides with existing state | 409 |
| 5 | `upstream_error` — upstream provider failed or was unreachable | 502 |
| 6 | `internal` — gateway failure | 500 |
| 7 | `payload_too_large` — the request body exceeds the management body limit | 413 |
| 8 | `method_not_allowed` — the method is not registered on the addressed path | 405 |
| 9 | `unsupported_media_type` — the request body's content type is not JSON | 415 |

Request bodies are parsed strictly as JSON and must carry the `Content-Type: application/json` header; a request without it is rejected with errno 9. Every failure inside the management API version router renders this body — a rejected field, a malformed JSON body, an oversized body, a wrong body content type, an unmatched method, an unmapped path, a storage or upstream failure — so no response shaped by axum itself ever escapes the version's error domain.

The rest of this document names management errors by category and HTTP status (e.g. 404 `not_found`). Upstream failures on management endpoints always render errno 5 with HTTP 502 and the upstream status embedded in the message; a foreign upstream status is never forwarded verbatim. Failures whose cause is internal — engine failures, constraint details, lease failures, stored drift (errno 6, and constraint-violation 409s) — render their category's fixed message, so the cause is recorded in the log record and never reaches the caller. The outward OpenAI error taxonomy (§6.7) is an entirely separate error domain, with its own error type, and is never rendered on a management route.

One strictness rule applies uniformly to every management request body: **names are non-empty**. Every request field naming an entity — the `name` a provider is created with, `upstream_id`, and `id_override` when present — must be a non-empty string; an empty name is rejected with 400 naming the field instead of falling through to a 404 lookup miss. Name content is the caller's choice and is never validated further. `providers.name` additionally carries a `CHECK (name <> '')` constraint, so the empty string is a storage-level violation (§3). Existing providers are addressed exclusively by their `provider_id`; an unknown id is rejected with 404.

### 7.0 Design Rules

The route surface follows seven rules. Every future route is designed against them, so they are normative, not descriptive:

1. **One action per route.** A route's path names the resource collection and the action — `/providers:create`, `/oauth:start` — and its payload is flat. A request never selects a mode through a combination of fields; create-and-update and authorize-or-create endpoints do not exist.
2. **Fields are two-state, never three.** A field is either **always present** — required, and nullable only when it mirrors a nullable storage column (`expires_at`, `id_override`) — or **absent-keeps** — omissible, and never `null`. A field that can be absent, `null`, and a value at the same time does not exist. The bindings render an always-present field as a required key (`T | null` for the nullable ones), and an absent-keeps field as `T?` — never `T | null | undefined`. One tolerance is explicit rather than accidental: for a required-nullable field, an omitted key parses as `null` (serde routes both spellings through one path, and under full-state semantics they share the single meaning "no value"), so the required key is a typing obligation on clients, not a parser rejection.
3. **Updates are full-state writes.** A `*:update` request states the complete desired readable state of its row; omitting a required field is a 400, not a keep. The only exception is write-only data — `api_key` and OAuth `credentials` — which is absent-keeps (absent keeps the stored secret, present replaces it) because it is never readable back.
4. **Variants are discriminated by type.** A payload whose shape varies by provider type is a `type`-tagged union with one variant per type, each carrying exactly the fields that type has — never a flat struct of optional fields whose validity a sibling field pins.
5. **Requests reference entities as `<entity>_id`.** A request that addresses an existing row names it by its own identifier — `provider_id`, `api_key_id`, `model_id` — on every route. The bare `id` appears only as a response row's own identifier.
6. **Every route answers `POST` with a JSON body.** No method carries semantics — reads and writes are distinguished by the action name, and a read's filters ride the body as typed fields instead of a query string. A no-argument request carries an empty `{}` object body.
7. **Responses carry only what the caller cannot know.** A response states assigned identity (a newly created row's `id`), server-derived values (`expires_at`, `authorize_url`), and state recomputed inside the write's transaction (composed public model IDs). A request's own restated state and facts the caller already holds are never echoed — so `providers:update` and `providers:delete` answer `{}`, and `oauth:reauthorize` answers exactly `{"expires_at"}`.

| Method | Path | Description |
| :--- | :--- | :--- |
| `POST` | `/management-api/v0/providers:list` | List all configured providers and their status |
| `POST` | `/management-api/v0/providers:create` | Create a static-key provider |
| `POST` | `/management-api/v0/providers:update` | Rewrite one provider's state |
| `POST` | `/management-api/v0/providers:delete` | Delete a provider and its models |
| `POST` | `/management-api/v0/api-keys:list` | List all gateway API keys |
| `POST` | `/management-api/v0/api-keys:create` | Create a gateway API key |
| `POST` | `/management-api/v0/api-keys:update` | Rewrite one gateway API key |
| `POST` | `/management-api/v0/api-keys:delete` | Delete a gateway API key |
| `POST` | `/management-api/v0/oauth:start` | Generate an OAuth authorize URL and state |
| `POST` | `/management-api/v0/oauth:complete` | Exchange an auth code and create a provider |
| `POST` | `/management-api/v0/oauth:reauthorize` | Exchange an auth code and replace a provider's credentials |
| `POST` | `/management-api/v0/models:list` | List every model row and its effective status |
| `POST` | `/management-api/v0/models:discover` | Refresh a provider's models from upstream |
| `POST` | `/management-api/v0/models:create` | Manually add a model row, or claim a vanished one |
| `POST` | `/management-api/v0/models:update` | Rewrite one model row's writable columns |
| `POST` | `/management-api/v0/models:delete` | Delete a model row |

### 7.1 Endpoint Contracts

- **`POST /providers:list`** — request `{}`. Response `{"providers": [<provider>]}`, ordered by name. A provider is a union discriminated by `type` (rule 4): `codex_oauth` and `antigravity_oauth` render `{"id", "name", "model_prefix", "is_enabled", "type", "expires_at"}`; `openai_response`, `openai_chat_completion`, and `gemini_generate_content` render `{"id", "name", "model_prefix", "is_enabled", "type", "base_url"}`. No provider renders another type's fields — a field never appears as a `null`-when-not-applicable placeholder — and the one nullable field, `expires_at`, mirrors its nullable storage column: `null` is the not-fresh state of a hand-replaced credential (§3), never an inapplicability marker. Credentials are never returned.
- **`POST /providers:create`** — request `{"type", "name", "model_prefix", "is_enabled", "base_url", "api_key"}`, every field required: the request is the complete provider state (rule 3). The `type` names the provider's static-key kind outright — one of `openai_response`, `openai_chat_completion`, or `gemini_generate_content`; an OAuth kind is rejected with 400, because OAuth account kinds are created by `oauth:complete`, which is the only path that mints their credentials. A `name` that is already taken is rejected with 409. Response `{"provider": <provider>}`: the server-assigned `id` is the fact the caller cannot know (rule 7).
- **`POST /providers:update`** — request is a union discriminated by the restated `type`, one variant per provider kind (rule 4):
  - `openai_response`, `openai_chat_completion`, `gemini_generate_content`: `{"provider_id", "type", "name", "model_prefix", "is_enabled", "base_url", "api_key"?}`
  - `codex_oauth`: `{"provider_id", "type", "name", "model_prefix", "is_enabled", "credentials"?}` with `credentials` = `{"accessToken", "refreshToken", "accountId"}`
  - `antigravity_oauth`: `{"provider_id", "type", "name", "model_prefix", "is_enabled", "credentials"?}` with `credentials` = `{"accessToken", "refreshToken", "projectId"}`

  The request states the provider's complete desired readable state (rule 3); `api_key`/`credentials` is the write-only absent-keeps exception — absent keeps the stored secret, present replaces it and resets `expires_at` (storing no expiry, which the gateway treats as not-fresh, §3). The restated `type` must equal the stored, immutable one — a mismatch is rejected with 400 naming the field — and doubles as a guard against updating the wrong row. An unknown `provider_id` yields 404; a `name` already held by another provider yields 409. The whole update — the provider row (a differing `name` renames it), any credentials replacement, and the public IDs of unoverridden models recomputed as `model_prefix + upstream_id` — applies in one transaction; a taken name or any recomputed ID colliding with an existing model row (of any provider) rejects the whole update with 409 and writes nothing. Response `{}`: the request already states the full readable state, so there is nothing the caller does not know (rule 7).
- **`POST /api-keys:list`** — request `{}`. Response `{"api_keys": [<api_key>]}`, ordered by row id, where an api key is `{"id", "name", "key", "is_enabled", "created_at"}`: the row's complete readable state, plaintext secret included — the administrator needs the secret to configure clients.
- **`POST /api-keys:create`** — request `{"name", "key", "is_enabled"}`, every field required: the request is the complete key state (rule 3) except the server-assigned identity. An empty `name` or `key` is rejected with 400 naming the field; a `key` another row already holds is rejected with 409 without naming the value — the secret never reaches a rendered body. Response `{"api_key": <api_key>}`: the server-assigned `id` and `created_at` are the facts the caller cannot know (rule 7).
- **`POST /api-keys:update`** — request `{"api_key_id", "name", "key", "is_enabled"}`, every field required: the request is the complete desired state of the row (rule 3) — a client renders the row, edits it, and sends the same shape back. The whole rewrite applies in one transaction that re-reads the row; an unknown `api_key_id` yields 404 and a `key` held by another row yields 409, and nothing is written. Response `{}`: the request already states the full readable state (rule 7).
- **`POST /api-keys:delete`** — request `{"api_key_id"}`; deletes the key, which stops authorizing traffic on the next request. An unknown id yields 404. Response `{}` (rule 7: deletion produces no readable fact).
- **`POST /providers:delete`** — request `{"provider_id"}`; deletes the provider and, via cascade, its models only (`oauth_states` rows are provider-agnostic and not attached to a provider, so a state completed after a deletion legitimately recreates the provider — see §3). An unknown id yields 404. Response `{}` (rule 7: deletion produces no readable fact).
- **`POST /oauth:start`** — request `{"type"}` (`codex_oauth` or `antigravity_oauth`; other values are rejected by the field's own parsing). Sweeps expired `oauth_states` rows, persists the new state row, and responds `{"authorize_url", "state", "expires_at"}` — three server-derived facts, the expiry in microseconds.
- **`POST /oauth:complete`** — request `{"state", "code", "name", "model_prefix"}`, every field required: the flow creates a provider, and naming is stated in full (rule 3). The empty-name 400, the empty-or-malformed-code 400, and the taken-name 409 are computed from the still-unconsumed state and fire before the state is consumed, so the pending flow survives a rejected completion. An unknown, expired, or already-consumed state yields 404 (§7.2). The state's token exchange runs (resolving the project ID for `antigravity_oauth`, §5.2), and the provider is created enabled with the supplied naming. A `name` taken concurrently is refused by the database's UNIQUE constraint as 409. Response `{"provider": <provider>}`: the new row's `id` and `expires_at` are the exchange's products.
- **`POST /oauth:reauthorize`** — request `{"state", "code", "provider_id"}`, every field required. Pre-consume rejections — an unknown `provider_id` (404), an empty or malformed `code` (400), and a state whose flow type differs from the provider's stored type (409) — fire before the state is consumed. The state is then consumed (404 when unknown, expired, or already consumed), the code is exchanged, and credentials and `expires_at` are replaced. A provider deleted between the lookup and the credentials write (a concurrent `providers:delete`) fails with 404 — mirroring the refresh path's vanished-provider classification — instead of a silent no-op write and a storage-drift 500. Response `{"expires_at"}`: the new expiry is the only readable fact the exchange produced; the new credentials are write-only (rule 7).
- **`POST /models:list`** — request `{}` or `{"provider_id"}`; the filter narrows the listing to one provider's rows and is never `null` (rule 2) — an absent field lists every provider, and an unknown `provider_id` yields an empty list, because a filter addresses no row. Response `{"models": [<model>]}`, ordered by public ID, where a model is `{"id", "provider_id", "provider", "upstream_id", "public_id", "id_override", "is_enabled", "discovery_state", "is_effectively_enabled"}`: `id` is the row identifier (`AUTOINCREMENT`, never recycled — §3) that `models:update` and `models:delete` address; `public_id` is the routing string clients address; `provider_id` is the owning provider's row identifier and `provider` its current name; `is_enabled` is the model's configured switch; `discovery_state` is the row's lifecycle state (§3); `is_effectively_enabled` is the three-input definition (§3) — a `vanished` row can carry `is_enabled: true` and still report `false`, so a badge reads `discovery_state`; `id_override` is the nullable mirror column (rule 2). Every row is listed, `vanished` included.
- **`POST /models:discover`** — request `{"provider_id"}` (unknown ids yield 404); refreshes the model list from upstream (§5.1/§5.2/§5.3). An upstream that reports no models — a fresh pre-onboarded account or an outage answering 200 with an empty payload — fails the refresh with 502 `upstream_error` before the storage layer is reached, so curated rows are never wiped or mass-vanished by a silent empty discovery. The apply runs in one transaction that re-reads the provider row, so public IDs compose from the provider's current prefix even when a concurrent `providers:update` changed it after the call's outer read: first every `discovered` row of the provider is marked `vanished`, then each listed identifier is upserted — an existing row of any state has only its `discovery_state` flipped to `discovered` (a listed `manual` row flips too), an absent row is inserted fresh (`discovered`, `is_enabled = 1`, no override, `public_id` composed as `id_override ?? (model_prefix + upstream_id)`). Existing rows are never rewritten beyond that state column — the write-permission invariant (§3): `id`, `public_id`, `id_override`, and `is_enabled` are never touched, no row is deleted, and an unlisted `manual` row is left alone. Each newly composed public ID is collision-checked against every row of the table — `vanished` and `manual` holders and `id_override` holders alike reserve their IDs — so two rows converging on one ID, or a new ID already routed by another row, rejects the whole refresh with an ID-specific 409 and writes nothing; batch-internal convergence is impossible (new rows carry no override over distinct identifiers), and `providers:update` keeps its own pairwise check. OAuth providers refresh their access token first (§3). Response `{"models": [<model>]}` — the provider's complete fresh rows, every state included: which `upstream_id`s appeared, which rows vanished, and how the public IDs composed are transaction-recomputed facts the caller cannot know (rule 7).
- **`POST /models:create`** — request `{"provider_id", "upstream_id", "id_override", "is_enabled"}`, every field required: the complete desired initial state of one manually added row (rule 3); `id_override` is the required-nullable mirror column (rule 2). The row is addressed by its natural key `(provider_id, upstream_id)`, with three outcomes: an absent identifier becomes a brand-new `manual` row with the requested state and a prefix-composed public ID; a `vanished` row is **claimed** — the manual recovery path: the row keeps its `id`, the requested `id_override`/`is_enabled` apply, the `public_id` recomposes, and the state becomes `manual`, so the claim resumes service exactly per the requested `is_enabled`; a row the upstream currently lists (`discovered`, or an existing `manual` row) yields 409 and writes nothing. An unknown `provider_id` yields 404, and a composed ID already routed by any other model row yields 409. Response `{"model": <model>}` — the composed `public_id` and the three-input `is_effectively_enabled` derive from transaction-time state, so they are not client-computable (rule 7).
- **`POST /models:update`** — request `{"model_id", "id_override", "is_enabled"}`, every field required: the complete state of the row's two writable columns (rule 3), addressed by row identifier like `api-keys:update`; `id_override` is the required-nullable field (rule 2). The route writes only `id_override` and `is_enabled` — it never touches `discovery_state` — so it cannot change a row's state and **cannot restore a vanished row**: the only recovery paths are an upstream reappearance (§3) and a `models:create` claim. An unknown `model_id` yields 404; the row lookup, the public-ID recomposition from the prefix re-read inside the transaction, the collision check, and the write apply in one transaction, and an ID already routed by another model row yields an ID-specific 409 with nothing written. Response `{"model": <model>}` — the recomposed `public_id` and three-input `is_effectively_enabled` derive from transaction-time state, so they are not client-computable (rule 7).
- **`POST /models:delete`** — request `{"model_id"}`; deletes the row. Deletion is administrator-only: discovery never deletes a row, and a deleted `discovered` row is rebuilt by the next discovery as a new row with a fresh, never-recycled identity (§3). An unknown id yields 404. Response `{}` (rule 7: deletion produces no readable fact).

### 7.2 OAuth Flow Semantics

- OAuth state rows expire 10 minutes after creation; completion of an unknown, expired, or already-consumed state yields 404.
- The state row is consumed by the first completion attempt that observes it — including attempts whose token exchange or project resolution then fails — so a failed exchange burns the state and a retry requires a fresh `oauth:start`. A completion rejected before consumption (an empty or taken creation name, an empty or malformed `code`, an unknown reauthorization `provider_id`, or a provider-type mismatch — see §7.1) does not consume the state; it remains available to a matching completion. A pasted `code` carrying callback-URL structure (`?`, `#`, `&`) or whitespace is such a pre-consume 400: only one opaque code value is ever exchanged.
- A refreshed or exchanged token pair stores credentials JSON per §3 and sets `expires_at` to the microsecond stamp `now + expires_in × 1_000_000` — the endpoint reports seconds, the database stores microseconds; when the token response omits `expires_in`, the `exp` claim of the access token JWT (seconds, scaled the same way) is used, falling back to the 86400-second default lifetime.

### 7.3 TypeScript Bindings

- The management API's wire surface is published as TypeScript bindings behind the `management-api-typescript-bindings` feature: `cargo run --bin generate-management-api-typescript-bindings --features management-api-typescript-bindings -- <output-directory>` renders every wire payload type declaration plus the route table into one file per version, directly in the output target. The repository's own consumer is the `web/` workspace: `npm run generate-bindings` inside `web/` regenerates `web/src/management-api/generated/` from this working tree's local sources (see `web/README.md` §7.1).
- ts-rs's serde-attribute notices are switched off for the bindings dependency (`no-serde-warnings`), because the two attributes this repository uses that ts-rs 12 cannot interpret are both load-bearing and neither changes a published shape: `deny_unknown_fields` on an internally tagged request enum is the version's unknown-field rejection, and `deserialize_with` on an absent-keeps field is what makes absence keep the stored value instead of accepting `null` — the field's own `ts(optional)` already states its published shape. The notices would be permanent noise rather than signal; in exchange, a serde attribute that does change the shape (`with`, `serialize_with`, `into`) must pair with an explicit `#[ts(as = "...")]`, since nothing warns about the mismatch any more.
- Bindings are versioned with the routes they describe: each version's bindings are written into their own `<version>.ts` file in the output target (`<output-directory>/v0.ts`), and every route table key is the route's path within that version (`/providers:list`), so the file name — not the key — carries the version. One file holds everything a client of the version needs: the type declarations and the route table reference each other inside it, with no imports.
- Each version also publishes the wire types it owns besides its route payloads: its error body (`ErrorResponse`, the flat `{errno, message}` object) and its errno taxonomy (`ErrorCode`), the latter as a numeric TypeScript enum whose values are the errno discriminants tabulated above.
- Every closed set the API publishes is unordered and renders as a plain TypeScript union — the provider kinds, the upstream formats, the discovery states — so the bindings carry no ordering, const-tuple, or enumeration artifact of any kind.
- The route table types the path, method, request, and response of every route (`Path`, `Method<P>`, `Request<P, M>`, `Response<P, M>`), so a client addresses a versioned route without restating it. Every route's declared request and response types are published, the empty request and response types of body-less-result routes included; a payload type with no declaration of its own is a fatal declaration error rather than a route table referencing a type the file never declares.
- The unions of §7.1 render as their `type`-discriminated TypeScript unions — a TypeScript client narrows a provider by its `type` with exhaustiveness checking — and the two-state rule (§7.0) holds in the bindings verbatim: an always-present field renders `field: T` (or `T | null` only for the nullable mirror columns), and an absent-keeps field renders `field?: T`, never `T | null | undefined`.

---

## 8. Configuration & Deployment

### Configuration File (`config.jsonc`)

JanusGate is configured through a JSON file supporting comments (JSONC), supplied via the `-c` or `--config` command-line argument. The root schema is: `log_level` (level filter, see below), `listen` (IP address and port, e.g. `"0.0.0.0:8080"`; an IP literal only — never a DNS name, because a listener address is a local binding fact, not a name to resolve), `tls` (optional; `certificate`, `private_key` — present serves HTTPS over TLS, absent serves plaintext HTTP), `database` (`path`, `max_connections`, `pool_timeout`), `management_api` (`token`, `cors`), and `api` (`cors`). Gateway API keys are database-managed (§7.1); no secret lives in configuration. Each `cors` object configures `allowed_origin` and `allow_private_network`: `allowed_origin` specifies allowed origins as a single string (e.g. `"*"` or `"https://dashboard.example.com"`) or an array of strings (e.g. `["*"]`, `["https://dashboard.example.com"]`, or `[]` to disallow all cross-origin requests), and `allow_private_network` is a boolean controlling Private Network Access (PNA) preflight responses (`Access-Control-Allow-Private-Network`). Refer to [`config.example.jsonc`](config.example.jsonc) for the configuration layout and available options.

Required fields state facts or secrets the gateway must never invent: `listen`, `database.path`, and `management_api.token`. Every other field carries a documented default and may be omitted: `log_level` (`info`), `database.max_connections` (4), `database.pool_timeout` (5000 ms), and each `cors` field (`allowed_origin` `[]` — no cross-origin requests; `allow_private_network` `false`); a whole `cors` object may be omitted as well.

### Configuration Loading, Errors, and Hot Reload

Configuration loading is one pipeline shared by startup and every reload: read the file, strip comments, deserialize strictly — unknown fields are rejected — and resolve the TLS material into a Rustls server configuration. A loading failure names its input and carries the deserializer's own message — the offending value with its line and column — verbatim; no hint layer sits on top. At startup a failure is fatal. Behind one global configuration object, an applied schema is deliberately leaked — it lives for the rest of the process, a few hundred bytes per reload, which is what lets every consumer read it as a plain dereference with no lock, guard, or copy — while the TLS server configuration is refcounted and swapped without accumulation. Consumers read the live state per request, per accepted connection, or per startup subsystem.

Once running, a watcher task (`notify`: inotify on Linux, FSEvents on macOS, ReadDirectoryChangesW on Windows, feeding a Tokio channel) watches the parent directories of the configuration file and of the currently configured TLS certificate and private key — parent directories, so editors that replace files by rename are seen. Reloads are debounced: an event opens a 500 ms quiet window, every further event restarts it, and one unconditional reload — read, parse, resolve, apply — runs once the inputs fall quiet, so an editor's write-rename chain or a certificate rotation writing several files never reaches the parser in an intermediate state. What a reload moves:

- `management_api.token` — per request: the management authentication middleware reads the live schema. Gateway API keys are database-managed (§7.1): a created, rotated, or disabled key takes effect on the next outward request with no reload involved.
- TLS material, including the presence of the `tls` section — per accepted connection: each connection is served with the configuration current at accept time; established connections are unaffected.
- `log_level` — immediately: the global log filter moves with the reload (see Logging).

Restart-only, because they are consumed when their subsystem is built at startup: `listen` (the bound socket), `database.*` (the open pool), and the two `cors` sections (the routers' CORS layers).

A reload that fails for any reason — an unreadable file, a rejected schema, TLS material that does not resolve — keeps the previously applied configuration and logs the failure at error level; a running gateway never dies over a reload. The schema and TLS cells swap together or not at all: a configuration whose TLS material does not resolve keeps the previous schema as well. A reload that changes the TLS paths re-arms the watches.

### Logging

- The global logger is installed once during startup, after the configuration is parsed and before every other subsystem is initialized.
- The minimum severity a log record must carry to be emitted is taken **exclusively** from the root `log_level` configuration value, one of `off`, `error`, `warn`, `info`, `debug`, `trace` (case-insensitive). Environment variables — `RUST_LOG` in particular — are never consulted, and no per-target directives exist. The installed filter is open; the effective severity is the global maximum level, set from `log_level` at startup and moved by every configuration reload (§8), so the severity follows the live configuration without reinstalling the logger.
- Records are written to standard error as plain text without ANSI styling, one line per record in the format `<RFC 3339 UTC timestamp (millisecond precision)> <level padded to 5 characters> <target>: <message>`, for example:

  ```text
  2026-09-11T08:15:30.123Z INFO  janusgate::server: listening on http://0.0.0.0:8080
  ```

- **Severity policy**:
  - `error` — an operation failed on the gateway's or an upstream's behalf: a request failed with an upstream or internal error (logged once at the response boundary that renders it, with the model and provider context the boundary holds), an upstream stream aborted mid-flight (logged where the pump detects the abort), the TCP accept loop failed an accept (retried with backoff), or a configuration reload failed (the previously applied configuration keeps serving).
  - `warn` — the gateway continues with degraded or anomalous-but-tolerated behavior: the best-effort `TCP_NODELAY` activation failed, a token-refresh lease could not be cleared after a failed refresh, an unauthorized management API request was rejected, or a configuration-watching step failed transiently (the watcher task keeps running).
  - `info` — lifecycle and state changes: a database brought to a schema version (a fresh database's initial application or an outdated one's migrations), the listening socket, a completed token refresh, a configuration or TLS material reload applied, and every management mutation (a provider created, updated, or deleted, an API key created, updated, or deleted, an OAuth flow started, completed, or reauthorized, a model created — including claiming a vanished row — a model discovery, a model update, a model deleted).
  - `debug` — per-request pipeline detail: the outward dispatch (public model, provider, upstream model, wire format, streaming mode), the upstream response status, client-fault rejections (malformed body, failed authentication, unknown model, conflicts), connection-edge anomalies (failed TLS handshakes and connections that end with errors), and unauthorized outward requests.
  - `trace` — reserved; the gateway emits no `trace` records.
- Every rendered API error is logged exactly once — at the response boundary rendering it, or, for failures that become mid-stream error frames instead of an HTTP error response, at the pump that detects them. Log records never carry credentials, tokens, OAuth authorization codes or state tokens, prompt text, or completion text, matching the stateless-execution design (§1).
- Records emitted by dependency crates (`reqwest`, `hyper`, …) pass through the same global filter and appear under their own targets; there is no per-target suppression.

### Running the Server
```bash
cargo build --release
./target/release/janusgate -c config.jsonc
```
