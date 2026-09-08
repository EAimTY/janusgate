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
- **Strict model passthrough**: Zero automatic or hardcoded model mapping. Upstream calls pass the exact model identifiers yielded by upstream model discovery (`upstream_name`), even for OAuth providers.
- **Excluded**: Multi-tenancy, dynamic pooling/load balancing, automatic provider fallback, automatic model mapping/aliasing, prompt caching, request logging, billing/rate-limiting, outbound proxying.

---

## 2. Technology Stack & Request Pipeline

### 2.1 Technology Stack

| Layer | Component | Notes |
| :--- | :--- | :--- |
| **Language** | Rust (2024 edition) | Strict types, memory safety, zero-cost abstractions |
| **HTTP Server** | `hserver` | Low-level Hyper-based HTTP server driving accept loop and connection lifecycle (plaintext HTTP, no TLS for now) |
| **Routing & Handlers** | `axum` (Router & utils) | Hierarchical routing (implements `tower::Service`), request extractors, and response helpers; `axum::serve` is strictly unused |
| **Middleware** | `tower` | Service abstraction, middleware composition, and layer stacking |
| **Database** | `turso` | In-process SQLite-compatible SQL database (`tursodatabase/turso`) |
| **HTTP Client** | `reqwest` | Connection pooling and streaming |
| **Serialization** | `serde` + `serde_json` | Strongly typed parsing and serialization |
| **Async Runtime** | `tokio` | Multi-threaded async I/O |

### 2.2 Server Pipeline Architecture & Router Hierarchy

```text
[Incoming TCP / Plaintext HTTP]
               │
               ▼
      ┌─────────────────┐
      │     hserver     │  (Accept loop & HTTP connection lifecycle; no TLS; axum::serve unused)
      └────────┬────────┘
               │ (invokes Router as tower::Service)
               ▼
      ┌─────────────────┐
      │   Root Router   │  (Fallback: generic 404 handler)
      └───┬─────────┬───┘
          │         │
          │         └──────────────────────────────────┐
          ▼                                            ▼
┌───────────────────┐                        ┌───────────────────┐
│ Management API Rtr│ (/management-api/*)    │    API Router     │ (/api/*)
│ (Management Auth) │                        │ (Fallback: 404)   │
│ (Fallback: 404)   │                        └─────────┬─────────┘
└───────────────────┘                                  │
                                                       ▼
                                             ┌───────────────────┐
                                             │ Format Sub-router │ (/openai-v1/*)
                                             │ (Format Auth)     │
                                             │ (Fallback: 404)   │
                                             └───────────────────┘
```

The server architecture separates low-level connection driving from hierarchical routing:
1. **Server Lifecycle (`hserver`)**:
   - `hserver` directly manages the TCP listener, socket accept loop, and low-level Hyper connection lifecycle over plaintext HTTP (TLS is omitted for now).
   - Receives the `Database` instance initialized at startup and supplies it to the root router as router state.
   - `axum::serve` is **strictly unused**. Instead, `hserver` delegates incoming HTTP requests directly to Axum's root `Router` as an implementation of `tower::Service`.
2. **Hierarchical Routers (`axum::Router`)**:
   - **Root Router**:
     - Holds the application state (`AppState` containing `Database`) and propagates it to sub-routers.
     - Nests `/management-api` and `/api` sub-routers.
     - Maintains its own top-level fallback route returning an HTTP 404 response for any unmapped root paths.
   - **Management API Sub-router (`/management-api`)**:
     - Scoped to management endpoints (`/list-providers`, `/save-provider`, etc.).
     - Wrapped with management authentication middleware (`Authorization: <token>`).
     - Maintains its own fallback route returning a management-specific 404 JSON error response.
   - **API Sub-router (`/api`)**:
     - Parent router for all external gateway protocols.
     - Nests isolated format sub-routers (e.g. `/openai-v1`).
     - Maintains its own fallback route returning an API-scoped 404 response for unmapped paths under `/api`.
   - **Format-Specific Sub-routers (e.g. `/openai-v1`)**:
     - Scoped to that protocol's endpoints (`/chat/completions`, `/responses`, `/models`).
     - Wrapped with format-specific authentication middleware (e.g. `Authorization: Bearer <key>`).
     - Maintains its own fallback route returning format-compliant 404 error payloads (e.g. OpenAI JSON error structure: `{"error": {"message": "Invalid URL", "type": "invalid_request_error", "code": null}}`).
3. **Independent 404 Fallback Routes**:
   - Every router at every level (root router, management API sub-router, API sub-router, and each format sub-router) defines its own dedicated fallback route returning an appropriately structured 404 response matching its respective domain.

---

## 3. Storage & Schema (Turso)

The database runs in-process via `turso` with SQLite-compatible syntax and strict column typing. `DATABASE.sqlite` is the canonical source of truth for the complete schema.

### Database Abstraction & Connection Pooling
- **Connection Pool**: Database access is managed through an in-process connection pool (`deadpool`), maintaining up to `database.max_connections` concurrent `turso::Connection` instances.
- **Implicit Prepared Statement Cache**: All queries and executions on both `Database` and `Transaction` pass through `prepare_cached()`, eliminating statement preparation overhead across pooled connections.
- **Eager Query Materialization**: `query()` drains every result set to completion and returns owned `Vec<Row>` values before the pooled connection is released. Lazily-consumed cursors are never exposed, so a result can never outlive its connection checkout, interleave with another borrower's statements, or leave unfinished statement state on a recycled connection.
- **Transaction Semantics**: A `Transaction<'db>` checks out and exclusively holds a pooled connection for its entire lifetime. Explicit `commit()` or `rollback()` consumes the transaction and returns the connection to the pool; dropping an uncommitted transaction flags the connection for Turso's automatic rollback, which is resolved through turso's dangling-transaction cleanup at the next pool checkout before the connection is reused. Connections that still report an open transaction at recycle time are discarded and replaced by the pool.
- **Error Domain**: The database module defines an `Error` enum using `thiserror` that directly wraps underlying Turso engine errors (`Turso`), alongside pool lifecycle errors (`PoolClosed`, `PoolTimeout`), filesystem I/O (`Io`), and invalid UTF-8 database path errors (`InvalidPath`). Connection pool errors with a backend cause are merged directly into `Turso`.
- **No Global Database Static**: The database is opened via `Database::open(path, max_connections)` during startup, yielding an owned `Database` instance that is passed to `server::run()` and injected into the Axum router hierarchy as shared state.
- **Schema Fingerprint Verification**:
  - During build time (`build.rs`), `DATABASE.sqlite` is canonicalized via `syntaqlite`, hashed with BLAKE3, and truncated to a non-zero 32-bit big-endian signed integer stored in `$OUT_DIR/database_schema_fingerprint`.
  - On startup, `Database::open()` opens the Turso database and inspects `PRAGMA user_version`:
    - **Fresh Database (`user_version == 0`)**: JanusGate executes the canonical DDL schema from `DATABASE.sqlite` together with the compiled schema fingerprint write inside a single atomic transaction, so a crash can never persist a partially applied schema.
    - **Compatible Database (`user_version == DATABASE_SCHEMA_FINGERPRINT`)**: The database schema is verified and startup proceeds.
    - **Incompatible Database (any other value)**: JanusGate panics immediately, failing fast to prevent execution against an invalid schema.

### Core Entities
- **Provider**:
  - `name`: 1–64 characters `[a-z0-9_-]`, unique, immutable identifier.
  - `model_prefix`: String prefixed to model names (e.g. `codex/` or `agy-`).
  - `type`: `openai_oauth`, `antigravity_oauth`, or `openai_compatible`.
  - `credentials`: Plaintext JSON payload (`{accessToken, refreshToken, accountId/projectId}` or `{apiKey}`) stored directly in the database without encryption.
  - `expires_at`: Unix timestamp when OAuth access token expires.
  - `refresh_started_at`: Lease timestamp to prevent concurrent token refresh races.
- **Model**:
  - Identified by `(provider_id, upstream_name)`.
  - `upstream_name`: Exact model identifier discovered from upstream; used verbatim for upstream requests without automatic mapping.
  - `name`: Public routing ID (`model_prefix + (name_override ?? upstream_name)`).
  - `enabled`: Effective status is `enabled_override ?? provider.enabled`.

---

## 4. Security & Authentication

1. **Inbound Authentication**:
   - Authentication is format-specific and enforced at the router layer via Tower middleware before requests reach endpoint handlers:
     - **Management Sub-router (`/management-api/*`)**: Uses management authentication middleware applied to the `/management-api` sub-router. Requires `Authorization: <token>` without any `Bearer` prefix, matching `management_api.token` in configuration.
     - **Outward API Sub-router (`/api/*`)**: Validates client requests against a shared gateway API key, matching `api.key` in configuration. Format-specific sub-routers enforce protocol-compliant header parsing independently (for example, the `/openai-v1` sub-router requires `Authorization: Bearer <key>` conforming to the OpenAI specification).
   - All token comparisons use constant-time byte comparison. Mismatch yields HTTP 401.
2. **Plaintext Credential Storage**:
   - Upstream credentials and tokens are stored directly in plaintext JSON within the database. No encryption or decryption is performed.
3. **Network Invariants**:
   - Inbound endpoints do not use sessions, cookies, or CORS headers.
   - Outbound requests to upstream providers connect directly without proxying.

---

## 5. Provider Upstream Specifics

### 5.1 OpenAI Codex OAuth (`openai_oauth`)
- **OAuth Constants**: Client ID `app_EMoamEEZ73f0CkXaXp7hrann`, PKCE S256, loopback redirect `http://localhost:1455/auth/callback`.
- **Upstream URL**: `https://chatgpt.com/backend-api/codex/responses`
- **Mandatory Request Invariants**:
  1. `store: false` must always be present.
  2. `stream: true` is strictly required by the upstream Codex endpoint. When a gateway client requests non-streaming mode (`"stream": false`), the gateway **must** request streaming upstream, buffer and aggregate the SSE chunks in memory, and return a single aggregated non-streaming JSON response.
  3. Required headers:
     ```text
     Authorization: Bearer <accessToken>
     chatgpt-account-id: <accountId>
     Originator: codex_cli_rs
     User-Agent: codex_cli_rs/0.147.0
     Session-Id: <uuid>
     Accept: text/event-stream
     ```

### 5.2 Google Antigravity OAuth (`antigravity_oauth`)
- **OAuth Constants**: Installed-app client ID & secret, loopback redirect `http://localhost:51121/oauth-callback`.
- **Upstream URL**: `https://daily-cloudcode-pa.googleapis.com/v1internal`
  - *Note*: Personal/consumer accounts must use `daily-cloudcode-pa.googleapis.com`; `cloudcode-pa.googleapis.com` returns `429 RESOURCE_EXHAUSTED`.
- **Upstream Actions**:
  - Non-streaming: `:generateContent`
  - Streaming: `:streamGenerateContent?alt=sse`
- **Envelope**:
  ```json
  {
    "project": "<projectId>",
    "model": "<upstream_name>",
    "request": <GenerateContentRequest>,
    "requestType": "agent",
    "userAgent": "antigravity",
    "requestId": "agent-<uuid>"
  }
  ```
- **Upstream Invariants**:
  - `thoughtSignature` blobs returned by upstream must be preserved and reattached across turns.
  - No automatic model mapping or tiered rewriting is performed; upstream calls pass the exact discovered `upstream_name`.

### 5.3 OpenAI Compatible (`openai_compatible`)
- User-specified `baseUrl`, `apiKey`, and `protocol` (`openai_response`, `openai_chat_completion`, or `gemini_generate_content`).

---

## 6. Translation Architecture (IR)

All incoming and outgoing payloads map through an internal Intermediate Representation (IR):

```text
[Client Request] ──> [Parse to IR] ──> [Render Upstream Payload] ──> [Upstream]
                                                                        │
[Client Response] <── [Render Client Format] <── [Parse IR Events] <─────┘
```

- **IR Request**: Messages (`role`, `parts: Text | Image | ToolCall | ToolOutput`), system instructions, tools, sampling (`temperature`, `top_p`, `max_output_tokens`), response format, reasoning effort.
- **IR Events**: `MessageStart`, `TextDelta`, `ReasoningDelta`, `ToolCallDelta`, `ToolCallDone`, `Finish(reason)`, `Usage`, `Done`.
- **SSE Parsing & Aggregation**:
  - Streaming clients receive live translated SSE chunks (`data: {...}\n\n`).
  - Non-streaming clients receive aggregated responses reconstructed from stream events.

---

## 7. Management API (`/management-api/*`)

All management routes are registered under the `/management-api` sub-router with its own 404 fallback handler and require authorization via `Authorization: <token>` (without any `Bearer` prefix).

| Method | Path | Description |
| :--- | :--- | :--- |
| `GET` | `/management-api/list-providers` | List all configured providers and their status |
| `POST` | `/management-api/save-provider` | Create or update a provider |
| `POST` | `/management-api/delete-provider` | Delete a provider and its models |
| `POST` | `/management-api/start-provider-oauth` | Generate OAuth authorize URL and PKCE state |
| `POST` | `/management-api/complete-provider-oauth` | Exchange auth code for tokens and save provider |
| `POST` | `/management-api/discover-models` | Refresh available models from upstream |
| `GET` | `/management-api/list-models` | List all routed models and their overrides |
| `POST` | `/management-api/save-model` | Update model overrides (name, enabled, thinking levels) |

---

## 8. Configuration & Deployment

### Configuration File (`config.jsonc`)

JanusGate is configured through a JSON file supporting comments (JSONC), supplied via the `-c` or `--config` command-line argument. Refer to [`config.example.jsonc`](config.example.jsonc) for the configuration layout and available options.

### Running the Server
```bash
cargo build --release
./target/release/janusgate -c config.jsonc
```
