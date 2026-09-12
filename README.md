# JanusGate — OpenAI-Compatible Gateway (Rust)

> Single source of truth for the system design and architecture. Contracts and invariants stated here are **normative**.

---

## 1. Overview & Scope

**JanusGate** is a self-hosted, lightweight AI gateway written in Rust. It exposes an OpenAI-compatible API (`/api/openai-v1`) backed by heterogeneous upstream accounts, managed via a management JSON API (`/management-api/*`).

### Provider Types

| Type | Identity | Upstream Wire Format |
| :--- | :--- | :--- |
| `openai_oauth` | ChatGPT Account | OpenAI Responses (`openai_response`) via Codex endpoint |
| `antigravity_oauth` | Google Account | Gemini GenerateContent (`gemini_generate_content`) via Google Code Assist |
| `openai_compatible` | Static API Key | `openai_response`, `openai_chat_completion`, or `gemini_generate_content` |

### Outward Protocols
- **Chat Completions**: `POST /api/openai-v1/chat/completions` (streaming SSE & non-streaming)
- **Responses**: `POST /api/openai-v1/responses` (streaming SSE & non-streaming)
- **Model Discovery**: `GET /api/openai-v1/models`

### Design Principles & Non-Goals
- **Stateless execution**: Zero persistence of prompts, completions, tokens, or usage metrics.
- **Headless admin**: No frontend SPA. Administration is conducted exclusively through `/management-api/*` JSON endpoints.
- **Strict model passthrough**: Zero automatic or hardcoded model mapping. Upstream calls pass the exact model identifiers yielded by upstream model discovery (`upstream_id`), even for OAuth providers.
- **Excluded**: Multi-tenancy, dynamic pooling/load balancing, automatic provider fallback, automatic model mapping/aliasing, prompt caching, request logging, billing/rate-limiting, outbound proxying.

---

## 2. Technology Stack & Request Pipeline

### 2.1 Technology Stack

| Layer | Component | Notes |
| :--- | :--- | :--- |
| **Language** | Rust (2024 edition) | Strict types, memory safety, zero-cost abstractions |
| **HTTP Server** | `hserver` + `socket2` | The gateway's `server` module owns the TCP listener (`socket2`) and the accept loop; `hserver` drives the per-connection Hyper HTTP/1 lifecycle (plaintext HTTP, no TLS for now) |
| **Routing & Handlers** | `axum` (Router & utils) | Hierarchical routing (implements `tower::Service`), request extractors, and response helpers; `axum::serve` is strictly unused |
| **Middleware** | `tower` | Service abstraction, middleware composition, and layer stacking |
| **Database** | `turso` | In-process SQLite-compatible SQL database (`tursodatabase/turso`) |
| **HTTP Client** | `reqwest` | Connection pooling and streaming |
| **Serialization** | `serde` + `serde_json` | Strongly typed parsing and serialization |
| **Logging** | `log` + `env_logger` | Log facade and stderr emission (§8); `env_logger` is built entirely from configuration, never from environment variables |
| **Async Runtime** | `tokio` | Multi-threaded async I/O |

### 2.2 Server Pipeline Architecture & Router Hierarchy

```text
[Incoming TCP / Plaintext HTTP]
               │
               ▼
      ┌─────────────────┐
      │server.rs+hserver│  (server.rs: TCP listener & accept loop; hserver: HTTP/1 lifecycle; no TLS)
      └────────┬────────┘
               │ (invokes Router as tower::Service)
               ▼
      ┌─────────────────┐
      │   Root Router   │  (Fallback: empty 404)
      └───┬─────────┬───┘
          │         │
          │         └──────────────────────────────────┐
          ▼                                            ▼
┌───────────────────┐                        ┌───────────────────┐
│ Management API Rtr│ (/management-api/*)    │    API Router     │ (/api/*)
│ (Management Auth) │                        │ (No fallback)     │
│ (Fallback: 404    │                        └─────────┬─────────┘
│   errno JSON)     │                                  │
└───────────────────┘                                  │
                                                       ▼
                                             ┌───────────────────┐
                                             │ Format Sub-router │ (/openai-v1/*)
                                             │ (Format Auth)     │
                                             │ (Fallback: OpenAI │
                                             │   404 JSON)       │
                                             └───────────────────┘
```

The server architecture separates low-level connection driving from hierarchical routing:
1. **Server Lifecycle (`server` module + `hserver`)**:
   - The gateway's own `server` module owns the TCP listener (`socket2`: dual-stack IPv4/IPv6, non-blocking, explicit listen backlog) and the accept loop, which retries failed accepts with exponential backoff and spawns one task per accepted connection; `hserver` only drives the low-level Hyper HTTP/1 connection lifecycle over plaintext HTTP (TLS is omitted for now).
   - Startup builds the shared `Context` (`src/context.rs`) — the `Database` handle plus the two upstream HTTP clients, data plane and control plane (§4) — and passes it to `server::run(context)`, which supplies it to the root router as router state.
   - `axum::serve` is **strictly unused**. Instead, every accepted HTTP/1 connection is delegated directly to Axum's root `Router` as an implementation of `tower::Service`.
2. **Hierarchical Routers (`axum::Router`)**:
   - **Root Router**:
     - Holds the shared `Context` — the `Database` handle plus the two upstream HTTP clients — as its router state and propagates it to sub-routers.
     - Nests `/management-api` and `/api` sub-routers.
     - Maintains its own top-level fallback route returning an empty HTTP 404 response for any unmapped root paths.
   - **Management API Sub-router (`/management-api`)**:
     - Scoped to management endpoints (`/list-providers`, `/save-provider`, etc.).
     - Wrapped with management authentication middleware (`Authorization: <token>`).
     - Maintains its own fallback route returning a management-specific 404 JSON error response.
   - **API Sub-router (`/api`)**:
     - Parent router for all external gateway protocols.
     - Nests isolated format sub-routers (e.g. `/openai-v1`).
     - Defines no fallback of its own: unmapped paths under `/api` propagate upward to the root router's empty 404.
   - **Format-Specific Sub-routers (e.g. `/openai-v1`)**:
     - Scoped to that protocol's endpoints (`/chat/completions`, `/responses`, `/models`).
     - Wrapped with format-specific authentication middleware (e.g. `Authorization: Bearer <key>`).
     - Maintains its own fallback route returning format-compliant 404 error payloads (e.g. OpenAI JSON error structure: `{"error": {"message": "Invalid URL", "type": "invalid_request_error", "code": null}}`).
3. **Independent 404 Fallback Routes**:
   - Exactly three fallback handlers exist, one per routing domain: the root router answers unmapped paths with an empty 404, the management API sub-router answers with its errno error JSON (§7), and the `/openai-v1` format sub-router answers with the OpenAI error JSON. The `/api` parent router defines none, delegating its unmatched paths to the root router.
4. **Method Mismatch on Routed Paths**:
   - A request whose method is not registered on an otherwise-matching routed path (e.g. `GET /api/openai-v1/chat/completions`) never reaches the 404 fallbacks: axum's method routing answers HTTP 405 with an empty body and an `Allow` header listing the registered methods.
   - This 405 is distinct from the 404 fallbacks above, which serve only unmapped paths; the router-level authentication middleware still applies, so an unauthorized request is rejected with 401 before the 405 is produced.

**Fail-fast panics are contractual**: the process installs a panic hook before any other startup operation that prints the panic diagnostic to stderr and terminates the process with exit code 1, so a panic in any spawned task — accepted-connection or request-pipeline — kills the whole gateway by design instead of leaving a partially operational server behind.

---

## 3. Storage & Schema (Turso)

The database runs in-process via `turso` with SQLite-compatible syntax and strict column typing. `src/database/SCHEMA.sqlite` is the canonical source of truth for the complete schema.

### Database Abstraction & Connection Pooling
- **Connection Pool**: Database access is managed through an in-process connection pool (`deadpool`), maintaining up to `database.max_connections` concurrent `turso::Connection` instances. When every connection is checked out, connection acquisition waits at most `database.pool_timeout` milliseconds before failing with `PoolTimeout`.
- **Connection Busy Timeout**: Every pooled connection and schema-bootstrap connection configures a 5-second busy timeout, so a write waits at most 5 seconds for the database file lock before failing with the underlying busy error.
- **Implicit Prepared Statement Cache**: All queries and executions on both `Database` and `Transaction` pass through `prepare_cached()`. Turso's statement cache is per-connection and persists across pool checkouts, so warm connections reuse their cached preparations; a freshly created pooled connection starts cold.
- **Eager Query Materialization**: `query()` drains every result set to completion and returns owned `Vec<Row>` values before the pooled connection is released. Lazily-consumed cursors are never exposed, so a result can never outlive its connection checkout, interleave with another borrower's statements, or leave unfinished statement state on a recycled connection.
- **Transaction Semantics**: A `Transaction` checks out and exclusively holds a pooled connection for its entire lifetime. Statement execution on the handle requires exclusive access (`execute()`/`query()` take `&mut self`), so statements within one transaction can never run concurrently or interleave. Explicit `commit()` or `rollback()` consumes the transaction and returns the connection to the pool; dropping an uncommitted transaction discards its connection instead of returning it to the pool, so the open engine-level write lock is released immediately rather than parked until the connection's next checkout. Connections that still report an open transaction at recycle time are discarded and replaced by the pool.
- **Error Domain**: The database module defines an `Error` enum using `thiserror` that directly wraps underlying Turso engine errors (`Turso`), alongside pool lifecycle errors (`PoolClosed`, `PoolTimeout`), filesystem I/O (`Io`), and invalid UTF-8 database path errors (`InvalidPath`). Connection pool errors with a backend cause are merged directly into `Turso`.
- **No Global Database Static**: The database is opened via `Database::open(path, max_connections, pool_timeout)` during startup, yielding an owned `Database` instance that startup wraps into the shared `Context` (§2.2) passed to `server::run()` and injected into the Axum router hierarchy as shared state.
- **Schema Fingerprint Verification**:
  - During build time (`build.rs`), `src/database/SCHEMA.sqlite` is canonicalized via `syntaqlite`, hashed with BLAKE3, and truncated to a non-zero 32-bit big-endian signed integer stored in `$OUT_DIR/database_schema_fingerprint`.
  - On startup, `Database::open()` opens the Turso database and inspects `PRAGMA user_version`:
    - **Fresh Database (`user_version == 0`)**: JanusGate executes the canonical DDL schema from `src/database/SCHEMA.sqlite` together with the compiled schema fingerprint write inside a single atomic transaction, so a crash can never persist a partially applied schema.
    - **Compatible Database (`user_version == DATABASE_SCHEMA_FINGERPRINT`)**: The database schema is verified and startup proceeds.
    - **Incompatible Database (any other value)**: JanusGate panics immediately, failing fast to prevent execution against an invalid schema.

### Core Entities
- **Provider**:
  - `id`: Integer primary key; the provider's public identifier. Every management reference to an existing provider — updating, renaming, deleting, discovering models, saving model overrides, re-authorizing — goes through it, and it never changes.
  - `name`: Non-empty, unique, renameable label. Its content — characters, case, length — is the administrator's choice and is never validated beyond non-emptiness; the gateway stores it verbatim. The column carries a `CHECK (name <> '')` constraint, so the empty string is a storage-level violation as well. Renaming is an ordinary `save-provider` update; the id and every model routing stay untouched.
  - `model_prefix`: Non-null string prefixed to model names (e.g. `codex/` or `agy-`); may be the empty string. Stored verbatim with no default: every write states the prefix explicitly.
  - `type`: `openai_oauth`, `antigravity_oauth`, or `openai_compatible`.
  - `credentials`: Plaintext JSON payload (`{accessToken, refreshToken, accountId/projectId}` or `{apiKey}`) stored directly in the database without encryption.
  - `expires_at`: Unix timestamp when the OAuth access token expires. Credentials replaced through `save-provider` are stored with no expiry, which the gateway treats as not-fresh: the first inference request then performs one lease-guarded refresh (see `refresh_started_at`) using the stored refresh token. A token response reporting a zero or negative `expires_in` is rejected as malformed (502), and a refresh grant rejected with the terminal `invalid_grant` error (RFC 6749 §5.2) fails with 502 whose message states the refresh token was rejected and the provider needs re-authorization — only user re-authorization recovers a revoked or expired grant, so re-attempting it is futile; the management errno remains the upstream-error category (502, §7) in both cases.
  - `refresh_started_at`: Lease timestamp to prevent concurrent token refresh races. When the token is expired, a request wins the lease by writing `refresh_started_at = now` inside a transaction conditional on the lease being absent or stale (older than 90 seconds); the winner refreshes the token and clears the lease. Concurrent requests poll up to 100 seconds — deliberately longer than the 90-second staleness window, so a lease abandoned by a crashed refresher is always reacquired rather than waited out — until the token is renewed, then re-read it; when the poll window elapses the request fails with 502. The lease acquire is a read-then-write transaction: when a concurrent commit invalidates its read snapshot, the engine refuses the write upgrade with the stale-snapshot busy error, which is classified as "lease not acquired" and joins the same poll loop, so a burst of simultaneous acquirers on one expired token waits its turn instead of failing. Any refresh failure — the grant or its persistence — clears the lease before the original error is returned, so waiters can acquire it instead of burning their whole wait behind a refresh that already failed; a failed lease clear never replaces that original error. The renewed credentials persist only while the refresher still owns the lease it acquired (`refresh_started_at` still equals the acquired stamp): a credentials replacement or stale-lease reacquisition that lands while the grant is in flight wins the row, the in-flight result — derived from superseded credentials — is discarded without touching the newer state, and the loser re-enters the lease competition.
  - `is_enabled`: Stored serving switch (`0` or `1`, default `1`). Master switch for all models of this provider.
- **Model**:
  - Identified by `(provider_id, upstream_id)`.
  - `upstream_id`: Exact model identifier discovered from upstream; used verbatim for upstream requests without automatic mapping.
  - `id`: Public routing ID (`id_override ?? (model_prefix + upstream_id)`), maintained at write time and unique across the gateway — two model rows can never route one out-facing ID.
  - `id_override`: Non-NULL explicitly sets the public routing ID directly, bypassing `model_prefix`; NULL computes the ID from `model_prefix + upstream_id`.
  - `is_enabled`: Stored serving switch (`0` or `1`, default `1`). The provider's `is_enabled` status acts as a master switch: effective serving requires both `provider.is_enabled` and `model.is_enabled` (`provider.is_enabled && model.is_enabled`). A disabled provider turns off all its models without exception.
  - `thinking_levels_override`: JSON array of canonical thinking levels (see §6.4); `NULL` means no filtering, pure passthrough.
- **OAuthState**:
  - Pending OAuth authorization flow (`oauth_states`).
  - `state`: Opaque random URL-safe token identifying the flow; returned to the administrator together with the authorize URL and required again at completion.
  - `type`: Provider type the flow is for (`openai_oauth` or `antigravity_oauth`).
  - `verifier`: PKCE code verifier for `openai_oauth`; empty string for `antigravity_oauth` (which authenticates with a client secret instead of PKCE).
  - `expires_at`: Unix timestamp after which the flow is invalid (10 minutes after creation).
  - `oauth_states` rows are provider-agnostic: they are keyed by the opaque `state` token and never attached to a provider, so a state completed after its provider was deleted legitimately recreates the provider.
  - State consumption is a single atomic `DELETE … RETURNING` statement, so exactly one concurrent completion can ever observe a state and a state can never complete twice. A rejected (expired or unknown) state consumes nothing, and expired rows are swept whenever `start-provider-oauth` runs; completion of an unknown, expired, or already-consumed state yields 404.

---

## 4. Security & Authentication

1. **Inbound Authentication**:
   - Authentication is format-specific and enforced at the router layer via Tower middleware before requests reach endpoint handlers:
     - **Management Sub-router (`/management-api/*`)**: Uses management authentication middleware applied to the `/management-api` sub-router. Requires `Authorization: <token>` without any `Bearer` prefix, matching `management_api.token` in configuration.
     - **Outward API Sub-router (`/api`)**: Performs no authentication itself and defines no fallback of its own; it only nests the format sub-routers, delegating its unmatched paths to the root router's empty 404 (§2.2). Each format-specific sub-router enforces authentication and protocol-compliant header parsing independently — the `/openai-v1` sub-router validates client requests against the shared gateway API key, matching `api.key` in configuration, via `Authorization: Bearer <key>` conforming to the OpenAI specification. The `Bearer` scheme is compared case-insensitively (RFC 9110 §11.1) while the key itself is compared byte-exactly; malformed header forms (missing space, tab separator, glued scheme, leading whitespace, trailing content) are rejected.
   - All token comparisons use constant-time byte comparison. Mismatch yields HTTP 401.
   - Error response bodies are router-domain specific:
     - Root router: plain HTTP 404 with an empty body.
     - Management API: `{"error": {"code": <numeric errno>, "message": <human-readable message>}}`; the closed errno taxonomy and each errno's pinned HTTP status are defined in §7.
     - Outward API (`/openai-v1` format fallback): OpenAI JSON error structure `{"error": {"message": ..., "type": ..., "param": null, "code": ...}}`; the fallback returns `type: "invalid_request_error"`, `code: null` and `message: "Invalid URL (<METHOD> <path>)"` with HTTP 404.
2. **Plaintext Credential Storage**:
   - Upstream credentials and tokens are stored directly in plaintext JSON within the database. No encryption or decryption is performed.
3. **Network Invariants**:
   - Inbound endpoints do not use sessions or cookies. Cross-Origin Resource Sharing (CORS) is configurable independently on the outward API sub-router (`/api`) and the administrative management API sub-router (`/management-api`) via `cors` (`allowed_origin`, `allow_private_network`) in configuration. Preflight `OPTIONS` requests are handled and matching CORS response headers are attached, including Private Network Access headers when enabled.
   - Accepted inbound TCP connections enable `TCP_NODELAY` (best-effort: a failure is logged and the connection is still served), so small streaming frames — SSE events in particular — are written immediately instead of stalling against Nagle's algorithm and the peer's delayed ACK.
   - Inbound HTTP/1 connections that do not finish sending request headers within 30 seconds are closed.
   - Inbound connections presenting the HTTP/2 connection preface are closed: only plaintext HTTP/1 is spoken.
   - Outbound requests to upstream providers connect directly without proxying.
4. **Resource Bounds** (value — where enforced — what it bounds):
   - Upstream connect timeout, 10 s — both upstream HTTP clients — TCP connection establishment to an upstream provider.
   - Upstream read idle timeout, 300 s — data-plane upstream HTTP client — silence gap between consecutive reads of an upstream body (reset after every successful read, so it never truncates a slow-but-flowing response; the data plane sets no total request timeout, so a long generation is never cut off).
   - Control-plane total timeout, 30 s — control-plane upstream HTTP client — total wall-clock budget of one short control-plane request (OAuth token exchange and refresh, model discovery, project onboarding §5), so a stalled request fails below the 90-second refresh-lease staleness window (§3) instead of holding its lease or the requesting client for minutes.
   - Inbound HTTP/1 header-read deadline, 30 s — HTTP/1 connection builder — time a client may take to finish sending request headers before the connection is closed.
   - TCP listen backlog, 32 — TCP listener setup — pending, not-yet-accepted connections the kernel queues before refusing new ones.
   - Request body caps, 64 MiB outward and 2 MiB management — axum body-limit layer on each sub-router — larger request bodies are rejected with HTTP 413.
   - SSE frame cap, 16 MiB — SSE frame parser — a single upstream SSE frame; a larger frame aborts the stream as malformed.
   - Channel buffers, 64 — between the upstream pump and the event renderer, and between the SSE renderer and the response body — backpressure between pipeline stages.
   - Upstream error-body reads, 4 KiB with 400-character excerpts — every upstream failure the gateway renders (gateway error mirroring §6.7, OAuth token endpoint rejections §5.1/§5.2, model discovery rejections §7.1) — memory held to carry an upstream failure message; success bodies are legitimate JSON of unbounded size and are read in full.
   - Accept-failure backoff, 10 ms doubling to a 1 s cap — TCP accept loop — retry delay after a failed accept attempt.
   - OAuth onboarding budget, 60 s — `onboardUser` long-running operation loop (§5.2) — total wall-clock time for project-ID resolution.

---

## 5. Provider Upstream Specifics

### 5.1 OpenAI Codex OAuth (`openai_oauth`)
- **OAuth Constants**: Client ID `app_EMoamEEZ73f0CkXaXp7hrann`, PKCE S256, loopback redirect `http://localhost:1455/auth/callback`.
  - PKCE generation: `code_verifier` is Base64URL (unpadded) of 64 random bytes; `code_challenge` is Base64URL (unpadded) of SHA-256 over the ASCII verifier bytes.
  - Authorize URL: `https://auth.openai.com/oauth/authorize?response_type=code&client_id=<client_id>&redirect_uri=http://localhost:1455/auth/callback&scope=openid%20profile%20email%20offline_access&code_challenge=<challenge>&code_challenge_method=S256&state=<state>&originator=codex_cli_rs`.
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
- **Project ID Resolution**: The Google Cloud Code Assist project ID (`projectId` credential) is resolved once during OAuth completion:
  1. `POST https://daily-cloudcode-pa.googleapis.com/v1internal:loadCodeAssist` with JSON body `{"metadata": {"ideType": "ANTIGRAVITY", "platform": "PLATFORM_UNSPECIFIED", "pluginType": "GEMINI"}}`. If the response carries `cloudaicompanionProject`, that is the project ID.
  2. Otherwise `POST https://daily-cloudcode-pa.googleapis.com/v1internal:onboardUser` with JSON body `{"tierId": <tier>, "metadata": {...}}` where `<tier>` is `paidTier.id` when present, else `currentTier.id`, else `free-tier`. The long-running operation is polled by repeating the request until `done` is true, within a 60-second total wall-clock budget checked before every attempt (each attempt is bounded by the control-plane client's total timeout, but a fixed attempt count alone bounds nothing); an exhausted budget fails the resolution. A completed operation carrying a terminal `error` payload (`google.longrunning` failure shape `{code, message, status}`) fails the resolution with that status and message. The project ID is `response.cloudaicompanionProject`.
  - Failure to resolve a project ID fails the OAuth completion.
- **Upstream URL**: `https://daily-cloudcode-pa.googleapis.com/v1internal`
  - *Note*: Personal/consumer accounts must use `daily-cloudcode-pa.googleapis.com`; `cloudcode-pa.googleapis.com` returns `429 RESOURCE_EXHAUSTED`.
- **Upstream Actions**:
  - Non-streaming: `:generateContent`
  - Streaming: `:streamGenerateContent?alt=sse`
- **Headers**: `Authorization: Bearer <accessToken>`, `Content-Type: application/json`, `User-Agent: antigravity`. Every Antigravity Code Assist call — project resolution (`loadCodeAssist`, `onboardUser`), model discovery (`fetchAvailableModels`), and inference — sends this header set.
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
  - Every Code Assist response — `:generateContent` JSON bodies and `alt=sse` SSE frames alike — wraps the GenerateContentResponse in a `response` envelope beside `traceId` and `metadata`; the gateway unwraps the envelope when present and equally accepts bare GenerateContentResponse payloads (as sent by Gemini-compatible upstreams).
  - `thoughtSignature` blobs returned by upstream must be preserved and reattached across turns.
  - No automatic model mapping or tiered rewriting is performed; upstream calls pass the exact discovered `upstream_id`.
- **Model Discovery**: `POST https://daily-cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels` with JSON body `{}`; the response payload's `models` object keys are the `upstream_id` values.

### 5.3 OpenAI Compatible (`openai_compatible`)
- User-specified `baseUrl`, `apiKey`, and `protocol` (`openai_response`, `openai_chat_completion`, or `gemini_generate_content`).
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
- `reasoning_effort`: canonical thinking level (see §6.4) or absent.

### 6.2 IR Events

- `MessageStart {id, model}` — identifies the completion; emitted exactly once, first.
- `TextDelta (delta)` — incremental assistant text.
- `ReasoningDelta (delta)` — incremental reasoning/summary text.
- `ToolCallDelta {index, id?, name?, arguments_delta}` — incremental tool call assembly; `index` is stable per call within the response.
- `ToolCallDone {index, call_id, name, arguments, thought_signature?}` — complete tool call.
- `Finish (reason)` — `Stop | Length | ToolCall | ContentFilter`.
- `Usage {input_tokens, output_tokens, cached_tokens?, cache_write_tokens?, reasoning_tokens?}` — token accounting. The three breakdown values are `None` when the upstream reports no token-detail objects (every cross-format upstream, and any Responses upstream that reports totals only).
- `Done` — terminal event; always the last event of every event stream, regardless of upstream shape. Parsers never emit it; the gateway appends it when the upstream response is fully consumed.

Every upstream response — SSE stream or JSON document — is parsed into an IR event stream; every client response — streamed or aggregated — is rendered from IR events. The parser is chosen by the upstream streaming mode the gateway requested — always streaming for `openai_oauth` (§5.1), otherwise the client's requested mode (§6.3) — never by response headers: the Codex upstream serves its 200 SSE streams without a `Content-Type` header. Non-streaming client responses are aggregated in memory from the same event stream (Text/Reasoning/ToolCall deltas → accumulated content, ToolCallDone → final calls, Finish, Usage; `MessageStart` contributes nothing — outward responses carry renderer-minted identifiers). When a tool-call done event carries no assembled arguments, the accumulated streamed deltas are used; a repeated ToolCallDone for an already-completed call index is ignored.

Upstream refusal text is delivered as assistant message text (`TextDelta`), so it reaches clients through the ordinary text channel: Chat Completions upstreams map `delta.refusal` and `message.refusal` (content text before refusal text when one delta carries both), and Responses upstreams map `response.refusal.delta` events and `refusal` message content parts exactly like `output_text` parts.

### 6.3 Format Matrix

| Outward Endpoint | Client Format | Upstream Formats |
| :--- | :--- | :--- |
| `POST /api/openai-v1/chat/completions` | OpenAI Chat Completions | all three |
| `POST /api/openai-v1/responses` | OpenAI Responses | all three |
| `GET /api/openai-v1/models` | OpenAI List Models | — |

| Provider Type | Upstream Format | Upstream Streaming |
| :--- | :--- | :--- |
| `openai_oauth` | OpenAI Responses | always streaming upstream (invariant §5.1); non-streaming clients get an aggregated response |
| `antigravity_oauth` | Gemini GenerateContent (envelope §5.2) | matches the client's requested mode |
| `openai_compatible` | per `protocol` column | matches the client's requested mode |

- Streaming clients receive live translated SSE chunks (`data: {json}\n\n`). Chat Completions streams end with a `data: [DONE]` sentinel; Responses streams end after the terminal `response.completed` or `response.incomplete` event. Every Responses SSE payload embeds its event name as a `"type"` field and a monotonically increasing `sequence_number`, so strict clients that discriminate on the JSON payload work without reading the `event:` line.
- Streaming Chat Completions deliver usage in a single terminal usage chunk immediately before `data: [DONE]`; when an upstream reports usage repeatedly, the last observation wins and renders once. Delta chunks carry no `usage` field at all — usage appears only in the final chunk.
- Responses `response.output_text.delta` and `response.output_text.done` events and `output_text` content parts always carry `logprobs: []` — empty when no logprobs are produced — because the official openai-python client requires the field.
- Every Responses usage object the gateway emits — in the `response.completed`/`response.incomplete` terminal events and in the aggregated non-streaming body — carries exactly `{input_tokens, input_tokens_details: {cached_tokens, cache_write_tokens}, output_tokens, output_tokens_details: {reasoning_tokens}, total_tokens}`: openai-python's `ResponseUsage` types all five fields, and every detail-object integer, as required without defaults, so a breakdown the upstream did not report renders zero-filled (`0`) while upstream-reported values ride through; `total_tokens` is the sum of `input_tokens` and `output_tokens`. The outward Chat Completions usage shape is unchanged (`prompt_tokens`/`completion_tokens`/`total_tokens`).
- Every Responses response object the gateway emits — the `response` field embedded in streaming events, the terminal `response.failed` frame, and the aggregated non-streaming body — carries `parallel_tool_calls: false`, `tool_choice: "auto"`, and `tools: []`: the official Response shape types these fields without defaults, and the statics truthfully describe the gateway (no parallel tool-call control, the model decides when to call tools, no tool definitions are echoed back).
- `Finish(Length)` and `Finish(ContentFilter)` render as `response.incomplete` carrying `incomplete_details.reason` (`max_output_tokens` / `content_filter`), both in the streaming terminal event and in the aggregated non-streaming response's `status`/`incomplete_details`; all other finishes render `response.completed` / `status: "completed"`.
- A turn whose upstream delivered function calls finishes with the canonical reason `ToolCall` — mixed text+tool-call turns included, matching official Chat Completions semantics: the Chat Completions upstream maps its native `finish_reason` (`tool_calls`, `function_call`); the Responses upstream reports `ToolCall` for a turn that delivered function calls under any status other than `incomplete` (the Responses status vocabulary has no tool-call value), including calls recovered from a terminal output array; the Gemini upstream reports `ToolCall` when the turn delivered `functionCall` parts under any finish reason other than `MAX_TOKENS` or a filter reason; a `finishReason` of `MALFORMED_FUNCTION_CALL`, however, is a failed generation — the turn aborts as an upstream failure under §6.7 instead of rendering a clean finish. Truncation and content-filter finishes keep their existing mapping.
- Malformed upstream SSE/JSON payloads abort the stream; clients that are already streaming receive a terminal `data: {"error": {...}}` frame (Chat Completions) or a `response.failed` event (Responses) with the translated error. This applies to malformed trailing frames flushed at end-of-stream as well: they abort the stream with an error frame instead of terminating cleanly. Upstream failures delivered inside a success transport abort the same way: an OpenAI-family stream frame bearing a top-level `error` object, or a 2xx JSON body carrying an error object, aborts with a terminal error frame / 502 instead of parsing as an empty success; a top-level `"error": null` is the normal success shape of the non-streaming Response object and parses normally — only a non-null `error` value aborts.
- The Responses upstream parser recovers message text, reasoning text, and function calls exactly once: streamed and terminal items are fallback sources consulted only when no live event delivered that kind of item, so an `openai_compatible` upstream that streams complete `output_item.done` items or only a terminal `response.completed`/`response.incomplete` event without live events still delivers its `output`-array text and function calls to the client exactly once.
- On a clean upstream end-of-stream the gateway flushes buffered parser state (e.g. pending Chat Completions tool calls) into the event stream before the terminal `Done` event; an aborted stream skips the flush. A clean end-of-stream that carried no finish reason (Chat Completions) or no terminal event (Responses/Gemini) degrades to a `Stop` finish, so streamed and aggregated renderings of the same stream agree.
- Upstream deadlines: two upstream HTTP clients share a 10-second connect timeout. The data-plane client enforces a 300-second read timeout that resets after every successful read of the upstream body and sets no total request timeout: a stalled upstream therefore aborts the request (mapped to 502 per §6.7) instead of hanging it indefinitely, while a slow-but-flowing response is never truncated. The control-plane client (token exchange and refresh, model discovery, project onboarding) bounds each request at 30 seconds total, so a stalled control-plane request fails below the 90-second refresh-lease staleness window (§3).

### 6.4 Thinking Levels

- Canonical thinking levels are `off`, `low`, `medium`, `high`, ordered by increasing reasoning strength.
- Client requests carry an effort in their native format: Chat Completions `reasoning_effort` and Responses `reasoning.effort` accept `minimal | low | medium | high`; `minimal` maps to the canonical level `off`. Gemini has no inbound notion — antigravity-compatible upstreams are Gemini only, so this applies to outbound only.
- Outbound mapping: OpenAI-family upstreams receive `minimal | low | medium | high` (`off` → `minimal`); Gemini upstreams receive `generationConfig.thinkingConfig.thinkingBudget` with `off` → `0`, `low` → `1024`, `medium` → `8192`, `high` → `24576`.
- `models.thinking_levels_override` filters the requested level: the requested canonical level is lowered to the nearest allowed level at or below it, or raised to the lowest allowed level when none is at or below it. When the client requests no level and an override is configured, the highest allowed level is used. A `NULL` override is pure passthrough: the client's request is forwarded verbatim, or no reasoning configuration is emitted when the client requested none.

### 6.5 Model Routing

- Client-visible model IDs are resolved against `models.id` (public routing ID). Resolution requires the model's effective enabled status (see §3) to be on; unknown or disabled models yield HTTP 404 with an OpenAI `model_not_found` error.
- The IR `model` is always the `upstream_id` of the resolved row; public IDs never reach an upstream.
- `GET /api/openai-v1/models` lists exactly the effectively enabled models as `{"object": "list", "data": [{"id": <public ID>, "object": "model", "created": 0, "owned_by": <provider name>}]}`; `created` is a fixed `0` because the gateway does not track model creation times.

### 6.6 Supported Request Fields

Requests are parsed strictly: unknown fields and known-but-unsupported fields are rejected with HTTP 400 `invalid_request_error` naming the offending field. The carve-outs accept and ignore extra fields. For history replay: Responses `message` input items accept and ignore `id` and `phase` (codex replays its parsed output items verbatim each turn); Responses `function_call` and `function_call_output` input items accept and ignore `id` and `status` (the gateway's own rendered `function_call` items carry `id` and `status`, so replaying them verbatim must parse); Responses `output_text` content parts accept and ignore `annotations` and `logprobs` (the gateway's own rendered `output_text` parts carry both, so replaying them verbatim must parse); and Chat Completions assistant messages accept and ignore `reasoning` and `reasoning_content`. Beyond history replay, the `strict` flag is accepted and ignored on Responses function tools, on Responses `text.format` `json_schema`, and on Chat Completions function tools and `json_schema`. Chat Completions validates assistant `tool_calls[].function.arguments` as a JSON object at parse time, in parity with Responses `function_call` items. Supported fields:

- **Chat Completions**: `model`, `messages`, `tools` (function tools only), `tool_choice`, `temperature`, `top_p`, `max_tokens` / `max_completion_tokens` (both set is rejected), `response_format` (`text`, `json_object`, `json_schema`), `reasoning_effort`, `stream`, `stream_options` (`{include_usage}` only).
- **Responses**: `model`, `input` (string or item array), `instructions`, `tools` (function tools only), `tool_choice`, `temperature`, `top_p`, `max_output_tokens`, `reasoning` (`{effort}`; `summary: "auto"` accepted, other values rejected), `stream`, `store` (must be absent or `false`), `text.format` (`text`, `json_object`, `json_schema`).
- **Message roles**: Chat Completions `system` / `developer` / `user` / `assistant` / `tool`; Responses input items `message`, `function_call`, `function_call_output`, and `reasoning` (reasoning items are accepted and ignored — they carry no replayable state in a stateless gateway).
- System/developer messages concatenate in encounter order into IR `instructions`.
- Responses system/developer message content parts must be text; an `input_image` part is rejected with HTTP 400 naming the field `content` (instructions have no image channel).
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
| Upstream transport failure / malformed upstream payload — a stalled or broken transport, a Gemini `MALFORMED_FUNCTION_CALL` finish, or an OpenAI-family 2xx body embedding an `error` object | 502 | OpenAI error, `type: "api_error"`, `code: "upstream_error"` |
| Gateway failure | 500 | OpenAI error object, `type: "api_error"`, `code: "internal"` |

Error payloads for the `/responses` endpoint use the same OpenAI error JSON structure.

---

## 7. Management API (`/management-api/*`)

All management routes are registered under the `/management-api` sub-router with its own 404 fallback handler and require authorization via `Authorization: <token>` (without any `Bearer` prefix).

Responses are JSON. Success responses use HTTP 200; errors use the structure from §4: `{"error": {"code": <errno>, "message": <human-readable message>}}`. The `code` is a numeric errno from a closed taxonomy, and each errno pins exactly one HTTP status — the two can never drift apart:

| errno | Category | HTTP Status |
| :--- | :--- | :--- |
| 1 | `invalid_request` — the request is malformed or unsupported | 400 |
| 2 | `unauthorized` — missing or invalid management token | 401 |
| 3 | `not_found` — unknown resource or path | 404 |
| 4 | `conflict` — the request collides with existing state | 409 |
| 5 | `upstream_error` — upstream provider failed or was unreachable | 502 |
| 6 | `internal` — gateway failure | 500 |

The rest of this document names management errors by category and HTTP status (e.g. 404 `not_found`). Upstream failures on management endpoints always render errno 5 with HTTP 502 and the upstream status embedded in the message; a foreign upstream status is never forwarded verbatim. The outward OpenAI error taxonomy (§6.7) is an entirely separate error domain, with its own error type, and is never rendered on a management route.

One strictness rule applies uniformly to every management request body: **names are non-empty**. Every request field naming an entity — the `name` a provider is created with, `upstream_id`, and `id_override` when present — must be a non-empty string; an empty name is rejected with 400 naming the field instead of falling through to a 404 lookup miss. Name content is the caller's choice and is never validated further. `providers.name` additionally carries a `CHECK (name <> '')` constraint, so the empty string is a storage-level violation (§3). Existing providers are addressed exclusively by their `id`; an unknown id is rejected with 404.

| Method | Path | Description |
| :--- | :--- | :--- |
| `GET` | `/management-api/list-providers` | List all configured providers and their status |
| `POST` | `/management-api/save-provider` | Create or update a provider |
| `POST` | `/management-api/delete-provider` | Delete a provider and its models |
| `POST` | `/management-api/start-provider-oauth` | Generate OAuth authorize URL and PKCE state |
| `POST` | `/management-api/complete-provider-oauth` | Exchange auth code for tokens and save provider |
| `POST` | `/management-api/discover-models` | Refresh available models from upstream |
| `GET` | `/management-api/list-models` | List all routed models and their overrides |
| `POST` | `/management-api/save-model` | Update model configuration (id, is_enabled, thinking levels) |

### 7.1 Endpoint Contracts

- **`GET /list-providers`** → `{"providers": [<provider>]}` where a provider object is `{"id", "name", "model_prefix", "type", "protocol", "base_url", "expires_at", "is_enabled"}`; `protocol`, `base_url`, and `expires_at` are `null` when not applicable; credentials are never returned.
- **`POST /save-provider`** — request `{"id"?, "name", "model_prefix", "type"?, "protocol"?, "base_url"?, "credentials"?, "is_enabled"?}`. `name` and `model_prefix` are required non-nullable strings on every request — creation and update alike — with no defaults: the caller always states the naming, and `model_prefix` may be the empty string but never absent. `id` selects the provider: absent creates one, present updates it.
  - Creation (`id` absent): `type` and `credentials` are required (`{"apiKey": "..."}` for `openai_compatible`); OAuth types cannot be created here — they are created by `complete-provider-oauth` (400 otherwise). `openai_compatible` requires `protocol` and `base_url`. A `name` that is already taken is rejected with 409.
  - Update (`id` present): an unknown `id` is rejected with 404. The update may rename the provider — a `name` different from the stored one renames it in place, keeping the id and every model routing; a `name` already held by another provider is rejected with 409. Apart from the always-required `name` and `model_prefix`, the request is patch-only — omitted fields keep their stored values, and optional fields cannot be cleared back to absent (tri-state clearing is not supported). The stored row pins `type`: an omitted `type` defaults to the stored one, and an explicit mismatching `type` is rejected with 400 naming the field. Supplying `credentials` replaces them and resets `expires_at`; with credentials present but no `type` on creation, the request is rejected with 400. The whole update — the provider row, any credentials replacement, and the public IDs of unoverridden models — applies in one transaction: the supplied `model_prefix` recomputes each of the provider's unoverridden models' `models.id` values as `model_prefix + upstream_id` (models with an explicit `id_override` retain their `id = id_override` untouched), and if any recomputed ID collides with an existing model row (of any provider, including another model of the same provider), the update is rejected with 409 and nothing is written.
  - Response: `{"provider": <provider>}` (same shape as `list-providers`).
- **`POST /delete-provider`** — request `{"id"}`; deletes the provider and, via cascade, its models only (`oauth_states` rows are provider-agnostic and not attached to a provider, so a state completed after a deletion legitimately recreates the provider — see §3). Response: `{"deleted": true}`; an unknown id yields 404.
- **`POST /start-provider-oauth`** — request `{"type"}` (`openai_oauth` or `antigravity_oauth`; others 400). Sweeps expired `oauth_states` rows, persists the new state row, and responds `{"authorize_url", "state", "expires_at"}`.
- **`POST /complete-provider-oauth`** — request `{"state", "code"}` plus exactly one completion form:
  - Creation: `{"name", "model_prefix"}` — both required, no defaults; the `name` must be free (409 otherwise) and non-empty.
  - Re-authorization: `{"id"}` — replaces the credentials of the provider `id` (404 unknown).
  - Supplying fields of both forms is rejected with 400 naming the offending field (`name` or `model_prefix` beside `id`).
  - Looks up the unexpired state (404 otherwise — the state is unknown, expired, or already consumed), exchanges `code` at the provider's token endpoint, and for `antigravity_oauth` resolves the project ID (§5.2).
  - Every form rejection, the taken-name 409, and the type-mismatch 409 (a re-authorization whose state's provider type differs from the provider's stored type) are computed from the still-unconsumed state and fire before the state is consumed, so the pending flow survives for a well-formed completion.
  - Re-authorization: credentials and `expires_at` are replaced; a provider deleted between the completion's lookup and the credentials write (a concurrent `delete-provider`) fails the completion with 404 — mirroring the refresh path's vanished-provider classification — instead of a silent no-op write and a storage-drift 500. Creation: the provider is created with the supplied `name` and `model_prefix`, enabled (`is_enabled = 1`).
  - The state row is consumed in both cases. Response: `{"provider": <provider>}`.
- **`POST /discover-models`** — request `{"id"}` (provider row identifier; unknown ids yield 404); refreshes the model list from upstream (§5.1/§5.2/§5.3). An upstream that reports no models — a fresh pre-onboarded account or an outage answering 200 with an empty payload — fails the refresh with 502 `upstream_error` instead of writing an empty replacement, so curated rows are never wiped by a silent empty discovery. The replace applies in one transaction that re-reads the provider row, so public IDs are composed from the provider's current prefix even when a concurrent `save-provider` changed it after the call's outer read; overrides carry over to the re-discovered rows inside the same snapshot. Existing model rows for the provider are replaced, keyed by `upstream_id`: newly discovered models default to `is_enabled = 1`; `id_override`, `is_enabled`, and `thinking_levels_override` carry over to re-discovered models from their previous stored rows and vanish with undiscovered ones; `id` is computed as `id_override ?? (model_prefix + upstream_id)`. The composed public IDs undergo the same collision checks as `save-provider` — two discovered rows converging on one ID, or an ID already routed by another model row, reject the whole refresh with an ID-specific 409 and write nothing. OAuth providers refresh their access token first (§3). Response: `{"discovered": <count>}`.
- **`GET /list-models`** → `{"models": [<model>]}` where a model object is `{"provider_id", "provider", "upstream_id", "id", "id_override", "is_enabled", "is_effectively_enabled", "thinking_levels_override"}`; `provider_id` is the owning provider's public identifier and `provider` its current name; `is_enabled` is the model's configured switch; `is_effectively_enabled` is whether the model currently serves traffic (`provider.is_enabled && model.is_enabled`); `null` denotes absent overrides. A corrupt stored `thinking_levels_override` is database drift and renders as 500 `internal` instead of being masked as an absent override.
- **`POST /save-model`** — request `{"provider_id", "upstream_id", "id_override"?, "is_enabled"?, "thinking_levels_override"?}`; an unknown `provider_id` yields 404; omitted fields keep their stored values; `null` clears optional override fields (`id_override`, `thinking_levels_override`); `is_enabled` is a boolean (`true`/`false`) updating the model's serving state; `id` is computed as `id_override ?? (provider.model_prefix + upstream_id)`. The row lookup, the public-ID collision check, and the write apply in one transaction, so an ID already routed by another model row yields an ID-specific 409 and nothing is written. Unknown provider/model rows yield 404; thinking levels must be a non-empty array over the canonical level set (§6.4), and the stored override is revalidated through the same parser when the saved row is rendered, so drift fails with 500 `internal`. Response: `{"model": <model>}`.

### 7.2 OAuth Completion Semantics

- OAuth state rows expire 10 minutes after creation; completion of an unknown, expired, or already-consumed state yields 404.
- The state row is consumed by the first completion attempt that observes it — including attempts whose token exchange or project resolution then fails — so a failed exchange burns the state and a retry requires a fresh `start-provider-oauth`. A completion rejected before consumption (a mixed or incomplete completion form, a taken creation name, an unknown re-authorization id, or a provider-type mismatch — see §7.1) does not consume the state; it remains available to a matching completion.
- A refreshed or exchanged token pair stores credentials JSON per §3 and sets `expires_at` to `now + expires_in`; when the token response omits `expires_in`, the `exp` claim of the access token JWT is used, falling back to 86400 seconds.

---

## 8. Configuration & Deployment

### Configuration File (`config.jsonc`)

JanusGate is configured through a JSON file supporting comments (JSONC), supplied via the `-c` or `--config` command-line argument. The root schema is: `log_level` (level filter, see below), `listen`, `database` (`path`, `max_connections`, `pool_timeout`), `management_api` (`token`, `cors`), and `api` (`key`, `cors`). Each `cors` object configures `allowed_origin` and `allow_private_network`: `allowed_origin` specifies allowed origins as a single string (e.g. `"*"` or `"https://dashboard.example.com"`) or an array of strings (e.g. `["*"]`, `["https://dashboard.example.com"]`, or `[]` to disallow all cross-origin requests), and `allow_private_network` is a boolean controlling Private Network Access (PNA) preflight responses (`Access-Control-Allow-Private-Network`). Refer to [`config.example.jsonc`](config.example.jsonc) for the configuration layout and available options.

### Logging

- The global logger is installed once during startup, after the configuration is parsed and before every other subsystem is initialized.
- The minimum severity a log record must carry to be emitted is taken **exclusively** from the root `log_level` configuration value, one of `off`, `error`, `warn`, `info`, `debug`, `trace` (case-insensitive). Environment variables — `RUST_LOG` in particular — are never consulted, and no per-target directives exist.
- Records are written to standard error as plain text without ANSI styling, one line per record in the format `<RFC 3339 UTC timestamp (millisecond precision)> <level padded to 5 characters> <target>: <message>`, for example:

  ```text
  2026-09-11T08:15:30.123Z INFO  janusgate::server: listening on 0.0.0.0:8080
  ```

- **Severity policy**:
  - `error` — an operation failed on the gateway's or an upstream's behalf: a request failed with an upstream or internal error (logged once at the response boundary that renders it, with the model and provider context the boundary holds), an upstream stream aborted mid-flight (logged where the pump detects the abort), or the TCP accept loop failed an accept (retried with backoff).
  - `warn` — the gateway continues with degraded or anomalous-but-tolerated behavior: the best-effort `TCP_NODELAY` activation failed, a token-refresh lease could not be cleared after a failed refresh, or an unauthorized management API request was rejected.
  - `info` — lifecycle and state changes: canonical schema application on a fresh database, the listening socket, a completed token refresh, and every management mutation (provider saved or deleted, OAuth flow started or completed, model discovery, model overrides saved).
  - `debug` — per-request pipeline detail: the outward dispatch (public model, provider, upstream model, wire format, streaming mode), the upstream response status, client-fault rejections (malformed body, failed authentication, unknown model, conflicts), connection-edge anomalies (HTTP/2 preface and unresponsive connections closed), and unauthorized outward requests.
  - `trace` — reserved; the gateway emits no `trace` records.
- Every rendered API error is logged exactly once — at the response boundary rendering it, or, for failures that become mid-stream error frames instead of an HTTP error response, at the pump that detects them. Log records never carry credentials, tokens, OAuth authorization codes or state tokens, prompt text, or completion text, matching the stateless-execution design (§1).
- Records emitted by dependency crates (`reqwest`, `hyper`, …) pass through the same global filter and appear under their own targets; there is no per-target suppression.

### Running the Server
```bash
cargo build --release
./target/release/janusgate -c config.jsonc
```
