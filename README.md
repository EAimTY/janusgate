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
- **Connection Pool**: Database access is managed through an in-process connection pool (`deadpool`), maintaining up to `database.max_connections` concurrent `turso::Connection` instances. When every connection is checked out, connection acquisition waits at most `database.pool_timeout` milliseconds before failing with `PoolTimeout`.
- **Implicit Prepared Statement Cache**: All queries and executions on both `Database` and `Transaction` pass through `prepare_cached()`, eliminating statement preparation overhead across pooled connections.
- **Eager Query Materialization**: `query()` drains every result set to completion and returns owned `Vec<Row>` values before the pooled connection is released. Lazily-consumed cursors are never exposed, so a result can never outlive its connection checkout, interleave with another borrower's statements, or leave unfinished statement state on a recycled connection.
- **Transaction Semantics**: A `Transaction` checks out and exclusively holds a pooled connection for its entire lifetime. Statement execution on the handle requires exclusive access (`execute()`/`query()` take `&mut self`), so statements within one transaction can never run concurrently or interleave. Explicit `commit()` or `rollback()` consumes the transaction and returns the connection to the pool; dropping an uncommitted transaction discards its connection instead of returning it to the pool, so the open engine-level write lock is released immediately rather than parked until the connection's next checkout. Connections that still report an open transaction at recycle time are discarded and replaced by the pool.
- **Error Domain**: The database module defines an `Error` enum using `thiserror` that directly wraps underlying Turso engine errors (`Turso`), alongside pool lifecycle errors (`PoolClosed`, `PoolTimeout`), filesystem I/O (`Io`), and invalid UTF-8 database path errors (`InvalidPath`). Connection pool errors with a backend cause are merged directly into `Turso`.
- **No Global Database Static**: The database is opened via `Database::open(path, max_connections, pool_timeout)` during startup, yielding an owned `Database` instance that is passed to `server::run()` and injected into the Axum router hierarchy as shared state.
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
  - `refresh_started_at`: Lease timestamp to prevent concurrent token refresh races. When the token is expired, a request wins the lease by writing `refresh_started_at = now` inside a transaction conditional on the lease being absent or stale (older than 90 seconds); the winner refreshes the token and clears the lease, while concurrent requests poll until the token is renewed, the lease is cleared, or the lease goes stale (then retry). A failed refresh clears the lease.
- **Model**:
  - Identified by `(provider_id, upstream_name)`.
  - `upstream_name`: Exact model identifier discovered from upstream; used verbatim for upstream requests without automatic mapping.
  - `name`: Public routing ID (`model_prefix + (name_override ?? upstream_name)`), maintained at write time.
  - `enabled`: Effective status is `enabled_override ?? provider.enabled`.
  - `thinking_levels_override`: JSON array of canonical thinking levels (see §6.4); `NULL` means no filtering, pure passthrough.
- **OAuthState**:
  - Pending OAuth authorization flow (`oauth_states`).
  - `state`: Opaque random URL-safe token identifying the flow; returned to the administrator together with the authorize URL and required again at completion.
  - `type`: Provider type the flow is for (`openai_oauth` or `antigravity_oauth`).
  - `verifier`: PKCE code verifier for `openai_oauth`; empty string for `antigravity_oauth` (which authenticates with a client secret instead of PKCE).
  - `expires_at`: Unix timestamp after which the flow is invalid (10 minutes after creation).
  - Exactly one provider may be completed per state; the row is deleted at completion, and expired rows are rejected with 404.

---

## 4. Security & Authentication

1. **Inbound Authentication**:
   - Authentication is format-specific and enforced at the router layer via Tower middleware before requests reach endpoint handlers:
     - **Management Sub-router (`/management-api/*`)**: Uses management authentication middleware applied to the `/management-api` sub-router. Requires `Authorization: <token>` without any `Bearer` prefix, matching `management_api.token` in configuration.
     - **Outward API Sub-router (`/api/*`)**: Validates client requests against a shared gateway API key, matching `api.key` in configuration. Format-specific sub-routers enforce protocol-compliant header parsing independently (for example, the `/openai-v1` sub-router requires `Authorization: Bearer <key>` conforming to the OpenAI specification).
   - All token comparisons use constant-time byte comparison. Mismatch yields HTTP 401.
   - Error response bodies are router-domain specific:
     - Root router: plain HTTP 404 with an empty body.
     - Management API: `{"error": {"message": <human-readable message>, "code": <machine-readable code>}}`; an unauthorized request carries `code: "unauthorized"`, an unknown path `code: "not_found"` (HTTP 404).
     - Outward API (both `/api` fallback and `/openai-v1` format fallback): OpenAI JSON error structure `{"error": {"message": ..., "type": ..., "param": null, "code": ...}}`; the `/openai-v1` fallback returns `type: "invalid_request_error"`, `code: null` and `message: "Invalid URL (<METHOD> <path>)"` with HTTP 404.
2. **Plaintext Credential Storage**:
   - Upstream credentials and tokens are stored directly in plaintext JSON within the database. No encryption or decryption is performed.
3. **Network Invariants**:
   - Inbound endpoints do not use sessions, cookies, or CORS headers.
   - Outbound requests to upstream providers connect directly without proxying.

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
  3. Required headers:
     ```text
     Authorization: Bearer <accessToken>
     chatgpt-account-id: <accountId>
     Originator: codex_cli_rs
     User-Agent: codex_cli_rs/0.147.0
     Session-Id: <uuid>
     Accept: text/event-stream
     ```
- **Model Discovery**: `GET https://chatgpt.com/backend-api/codex/models` with `Authorization: Bearer <accessToken>` and `chatgpt-account-id: <accountId>` headers. The response payload must contain an `items` or `models` array whose entries carry a `slug` string; each slug is one `upstream_name`. Any other shape fails discovery.

### 5.2 Google Antigravity OAuth (`antigravity_oauth`)
- **OAuth Constants**: Installed-app client ID `1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com`, client secret `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`, loopback redirect `http://localhost:51121/oauth-callback`. No PKCE; the client secret authenticates the exchange.
  - Authorize URL: `https://accounts.google.com/o/oauth2/v2/auth?access_type=offline&client_id=<client_id>&prompt=consent&redirect_uri=http://localhost:51121/oauth-callback&response_type=code&scope=<scopes space-joined>&state=<state>`.
  - Scopes: `https://www.googleapis.com/auth/cloud-platform`, `https://www.googleapis.com/auth/userinfo.email`, `https://www.googleapis.com/auth/userinfo.profile`, `https://www.googleapis.com/auth/cclog`, `https://www.googleapis.com/auth/experimentsandconfigs`.
  - Token exchange: `POST https://oauth2.googleapis.com/token` with form body `code=<code>&client_id=<client_id>&client_secret=<secret>&redirect_uri=<redirect>&grant_type=authorization_code`; response is JSON `{access_token, refresh_token, expires_in, ...}`.
  - Token refresh: same endpoint with form body `refresh_token=<refresh_token>&client_id=<client_id>&client_secret=<secret>&grant_type=refresh_token`.
- **Project ID Resolution**: The Google Cloud Code Assist project ID (`projectId` credential) is resolved once during OAuth completion:
  1. `POST https://daily-cloudcode-pa.googleapis.com/v1internal:loadCodeAssist` with JSON body `{"metadata": {"ideType": "ANTIGRAVITY", "platform": "PLATFORM_UNSPECIFIED", "pluginType": "GEMINI"}}`. If the response carries `cloudaicompanionProject`, that is the project ID.
  2. Otherwise `POST https://daily-cloudcode-pa.googleapis.com/v1internal:onboardUser` with JSON body `{"tierId": <tier>, "metadata": {...}}` where `<tier>` is `paidTier.id` when present, else `currentTier.id`, else `free-tier`. The long-running operation is polled by repeating the request until `done` is true; the project ID is `response.cloudaicompanionProject`.
  - Failure to resolve a project ID fails the OAuth completion.
- **Upstream URL**: `https://daily-cloudcode-pa.googleapis.com/v1internal`
  - *Note*: Personal/consumer accounts must use `daily-cloudcode-pa.googleapis.com`; `cloudcode-pa.googleapis.com` returns `429 RESOURCE_EXHAUSTED`.
- **Upstream Actions**:
  - Non-streaming: `:generateContent`
  - Streaming: `:streamGenerateContent?alt=sse`
- **Headers**: `Authorization: Bearer <accessToken>`, `Content-Type: application/json`, `User-Agent: antigravity`.
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
- **Model Discovery**: `POST https://daily-cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels` with JSON body `{}`; the response payload's `models` object keys are the `upstream_name` values.

### 5.3 OpenAI Compatible (`openai_compatible`)
- User-specified `baseUrl`, `apiKey`, and `protocol` (`openai_response`, `openai_chat_completion`, or `gemini_generate_content`).
- Upstream calls target `baseUrl` with `Authorization: Bearer <apiKey>` (`openai_response`, `openai_chat_completion`) or `x-goog-api-key: <apiKey>` (`gemini_generate_content`).
  - `openai_chat_completion`: `POST <baseUrl>/chat/completions`.
  - `openai_response`: `POST <baseUrl>/responses`.
  - `gemini_generate_content`: `POST <baseUrl>/models/<upstream_name>:generateContent` and `:streamGenerateContent?alt=sse`; the request body is the bare GenerateContentRequest (no Antigravity envelope).
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
- `Usage {input_tokens, output_tokens}` — token accounting.
- `Done` — terminal event; always the last event of every event stream, regardless of upstream shape. Parsers never emit it; the gateway appends it when the upstream response is fully consumed.

Every upstream response — SSE stream or JSON document — is parsed into an IR event stream; every client response — streamed or aggregated — is rendered from IR events. Non-streaming client responses are aggregated in memory from the same event stream (MessageStart → id/model, Text/Reasoning/ToolCall deltas → accumulated content, ToolCallDone → final calls, Finish, Usage).

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

- Streaming clients receive live translated SSE chunks (`data: {json}\n\n`). Chat Completions streams end with a `data: [DONE]` sentinel; Responses streams end after the `response.completed` event.
- Malformed upstream SSE/JSON payloads abort the stream; clients that are already streaming receive a terminal `data: {"error": {...}}` frame (Chat Completions) or a `response.failed` event (Responses) with the translated error.

### 6.4 Thinking Levels

- Canonical thinking levels are `off`, `low`, `medium`, `high`, ordered by increasing reasoning strength.
- Client requests carry an effort in their native format: Chat Completions `reasoning_effort` and Responses `reasoning.effort` accept `minimal | low | medium | high`; `minimal` maps to the canonical level `off`. Gemini has no inbound notion — antigravity-compatible upstreams are Gemini only, so this applies to outbound only.
- Outbound mapping: OpenAI-family upstreams receive `minimal | low | medium | high` (`off` → `minimal`); Gemini upstreams receive `generationConfig.thinkingConfig.thinkingBudget` with `off` → `0`, `low` → `1024`, `medium` → `8192`, `high` → `24576`.
- `models.thinking_levels_override` filters the requested level: the requested canonical level is lowered to the nearest allowed level at or below it, or raised to the lowest allowed level when none is at or below it. When the client requests no level and an override is configured, the highest allowed level is used. A `NULL` override is pure passthrough: the client's request is forwarded verbatim, or no reasoning configuration is emitted when the client requested none.

### 6.5 Model Routing

- Client-visible model names are resolved against `models.name` (public routing ID). Resolution requires the model's effective enabled status (see §3) to be on; unknown or disabled models yield HTTP 404 with an OpenAI `model_not_found` error.
- The IR `model` is always the `upstream_name` of the resolved row; public names never reach an upstream.
- `GET /api/openai-v1/models` lists exactly the effectively enabled models as `{"object": "list", "data": [{"id": <public name>, "object": "model", "created": 0, "owned_by": <provider name>}]}`; `created` is a fixed `0` because the gateway does not track model creation times.

### 6.6 Supported Request Fields

Requests are parsed strictly: unknown fields and known-but-unsupported fields are rejected with HTTP 400 `invalid_request_error` naming the offending field. Supported fields:

- **Chat Completions**: `model`, `messages`, `tools` (function tools only), `tool_choice`, `temperature`, `top_p`, `max_tokens` / `max_completion_tokens` (both set is rejected), `response_format` (`text`, `json_object`, `json_schema`), `reasoning_effort`, `stream`, `stream_options` (`{include_usage}` only).
- **Responses**: `model`, `input` (string or item array), `instructions`, `tools` (function tools only), `tool_choice`, `temperature`, `top_p`, `max_output_tokens`, `reasoning` (`{effort}`; `summary: "auto"` accepted, other values rejected), `stream`, `store` (must be absent or `false`), `text.format` (`text`, `json_object`, `json_schema`).
- **Message roles**: Chat Completions `system` / `developer` / `user` / `assistant` / `tool`; Responses input items `message`, `function_call`, `function_call_output`, and `reasoning` (reasoning items are accepted and ignored — they carry no replayable state in a stateless gateway).
- System/developer messages concatenate in encounter order into IR `instructions`.
- Usage is always included in aggregated Chat Completions responses and in Responses usage-bearing events; streaming Chat Completions responses emit the usage chunk only when `stream_options.include_usage` is set.

### 6.7 Error Mapping

| Situation | HTTP Status | Outward Body |
| :--- | :--- | :--- |
| Malformed / unsupported request | 400 | OpenAI error, `type: "invalid_request_error"`, `code: null` |
| Invalid API key | 401 | OpenAI error, `type: "invalid_request_error"`, `code: "invalid_api_key"` |
| Unknown or disabled model | 404 | OpenAI error, `type: "invalid_request_error"`, `param: "model"`, `code: "model_not_found"` |
| Upstream returned a non-2xx status | upstream status | OpenAI error, `type: "api_error"`, `code: "upstream_error"`, message carries the upstream status and body excerpt |
| Upstream transport failure / malformed upstream payload | 502 | OpenAI error, `type: "api_error"`, `code: "upstream_error"` |

Error payloads for the `/responses` endpoint use the same OpenAI error JSON structure.

---

## 7. Management API (`/management-api/*`)

All management routes are registered under the `/management-api` sub-router with its own 404 fallback handler and require authorization via `Authorization: <token>` (without any `Bearer` prefix).

Responses are JSON. Success responses use HTTP 200; errors use the structure from §4 with codes `unauthorized` (401), `invalid_request` (400), `not_found` (404), `conflict` (409), `upstream_error` (502), and `internal` (500). Provider `name` values must match `[a-z0-9_-]{1,64}`.

| Method | Path | Description |
| :--- | :--- | :--- |
| `GET` | `/management-api/list-providers` | List all configured providers and their status |
| `POST` | `/management-api/save-provider` | Create or update a provider |
| `POST` | `/management-api/delete-provider` | Delete a provider and its models |
| `POST` | `management-api/start-provider-oauth` | Generate OAuth authorize URL and PKCE state |
| `POST` | `/management-api/complete-provider-oauth` | Exchange auth code for tokens and save provider |
| `POST` | `/management-api/discover-models` | Refresh available models from upstream |
| `GET` | `/management-api/list-models` | List all routed models and their overrides |
| `POST` | `/management-api/save-model` | Update model overrides (name, enabled, thinking levels) |

### 7.1 Endpoint Contracts

- **`GET /list-providers`** → `{"providers": [<provider>]}` where a provider object is `{"name", "model_prefix", "type", "protocol", "base_url", "expires_at", "enabled"}`; `protocol`, `base_url`, and `expires_at` are `null` when not applicable; credentials are never returned.
- **`POST /save-provider`** — request `{"name", "type"?, "model_prefix"?, "protocol"?, "base_url"?, "credentials"?, "enabled"?}`.
  - Creation: `type` and `credentials` are required (`{"apiKey": "..."}` for `openai_compatible`); OAuth types cannot be created here — they are created by `complete-provider-oauth` (400 otherwise). `openai_compatible` requires `protocol` and `base_url`.
  - Update: only supplied fields change; `name` is the lookup key and `type` must match the existing row (409 otherwise). Supplying `credentials` replaces them and resets `expires_at`.
  - Response: `{"provider": <provider>}` (same shape as `list-providers`).
- **`POST /delete-provider`** — request `{"name"}`; deletes the provider and, via cascade, its models and OAuth state attachments. Response: `{"deleted": true}`; unknown names yield 404.
- **`POST /start-provider-oauth`** — request `{"type"}` (`openai_oauth` or `antigravity_oauth`; others 400). Persists an `oauth_states` row and responds `{"authorize_url", "state", "expires_at"}`.
- **`POST /complete-provider-oauth`** — request `{"state", "code", "name", "model_prefix"?}`.
  - Looks up the unexpired state (404 otherwise), exchanges `code` at the provider's token endpoint, and for `antigravity_oauth` resolves the project ID (§5.2).
  - Existing provider with matching type: credentials and `expires_at` are replaced. Existing provider with differing type: 409. New provider: created with the supplied `name` and `model_prefix` (default `""`), enabled.
  - The state row is consumed in both cases. Response: `{"provider": <provider>}`.
- **`POST /discover-models`** — request `{"provider": <name>}`; refreshes the model list from upstream (§5.1/§5.2/§5.3). Existing model rows for the provider are replaced in one transaction, keyed by `upstream_name`: `name_override`, `enabled_override`, and `thinking_levels_override` carry over to re-discovered models and vanish with undiscovered ones; `name` is recomputed as `model_prefix + (name_override ?? upstream_name)`. OAuth providers refresh their access token first (§3). Response: `{"discovered": <count>}`.
- **`GET /list-models`** → `{"models": [<model>]}` where a model object is `{"provider", "upstream_name", "name", "name_override", "enabled", "enabled_override", "thinking_levels_override"}`; `enabled` is the effective status; `null` denotes absent overrides.
- **`POST /save-model`** — request `{"provider", "upstream_name", "name_override"?, "enabled_override"?, "thinking_levels_override"?}`; the three override fields are written as supplied (`null` clears an override) and `name` is recomputed. Unknown provider/model rows yield 404; a public-name collision yields 409; thinking levels must be a non-empty array over the canonical level set (§6.4). Response: `{"model": <model>}`.

### 7.2 OAuth Completion Semantics

- OAuth state rows expire 10 minutes after creation; completion of an expired or unknown state yields 404.
- A refreshed or exchanged token pair stores credentials JSON per §3 and sets `expires_at` to `now + expires_in`; when the token response omits `expires_in`, the `exp` claim of the access token JWT is used, falling back to 86400 seconds.

---

## 8. Configuration & Deployment

### Configuration File (`config.jsonc`)

JanusGate is configured through a JSON file supporting comments (JSONC), supplied via the `-c` or `--config` command-line argument. Refer to [`config.example.jsonc`](config.example.jsonc) for the configuration layout and available options.

### Running the Server
```bash
cargo build --release
./target/release/janusgate -c config.jsonc
```
