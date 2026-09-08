# JanusGate — OpenAI-Compatible Gateway (Rust)

> Single source of truth for the system design and architecture. Contracts and invariants stated here are **normative**.

---

## 1. Overview & Scope

**JanusGate** is a self-hosted, lightweight AI gateway written in Rust. It exposes an OpenAI-compatible API (`/api/openai-v1`) backed by heterogeneous upstream accounts, managed via an administrative JSON API (`/panel-api/*`).

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
- **Headless admin**: No frontend SPA. Administration is conducted exclusively through `/panel-api/*` JSON endpoints.
- **Outbound proxy control**: Native outbound HTTP/SOCKS5 proxy support to ensure upstream calls bypass IP/datacenter restrictions (e.g. OpenAI Cloudflare WAF).
- **Excluded**: Multi-tenancy, dynamic pooling/load balancing, automatic provider fallback, prompt caching, request logging, billing/rate-limiting.

---

## 2. Technology Stack

| Layer | Component | Notes |
| :--- | :--- | :--- |
| **Language** | Rust (2024 edition) | Strict types, memory safety, zero-cost abstractions |
| **HTTP Server** | `hserver` | Hyper-based HTTP server with explicit accept-loop control |
| **Routing** | `const-router` | Compile-time string-key router with binary search dispatch |
| **Database** | `turso` | In-process SQLite-compatible SQL database (`tursodatabase/turso`) |
| **HTTP Client** | `reqwest` | Connection pooling, streaming, and native HTTP/SOCKS5 proxying |
| **Cryptography** | `aes-gcm` + `sha2` | AES-256-GCM for credentials at rest; constant-time SHA-256 bearer auth |
| **Serialization** | `serde` + `serde_json` | Strongly typed parsing and serialization |
| **Async Runtime** | `tokio` | Multi-threaded async I/O |

---

## 3. Storage & Schema (Turso)

The database runs in-process via `turso` with SQLite-compatible syntax and strict column typing. `DATABASE.sqlite` is the canonical source of truth for the schema.

```sql
CREATE TABLE providers (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    model_prefix TEXT NOT NULL DEFAULT '',
    type TEXT NOT NULL,
    protocol TEXT NULL,
    base_url TEXT NULL,
    encrypted_credentials BLOB NOT NULL,
    expires_at INTEGER NULL,
    refresh_started_at INTEGER NULL,
    enabled INTEGER NOT NULL DEFAULT 1
) STRICT;

CREATE TABLE models (
    provider_id INTEGER NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
    upstream_name TEXT NOT NULL,
    name TEXT NOT NULL UNIQUE,
    name_override TEXT NULL,
    enabled_override INTEGER NULL,
    thinking_levels_override TEXT NULL,
    PRIMARY KEY (provider_id, upstream_name)
) STRICT, WITHOUT ROWID;

CREATE TABLE oauth_states (
    id INTEGER PRIMARY KEY,
    type TEXT NOT NULL,
    state TEXT NOT NULL UNIQUE,
    verifier TEXT NOT NULL,
    provider_id INTEGER NULL REFERENCES providers(id) ON DELETE CASCADE,
    expires_at INTEGER NOT NULL
) STRICT;

CREATE INDEX idx_oauth_states_expires_at ON oauth_states(expires_at);
```

### Core Entities
- **Provider**:
  - `name`: 1–64 characters `[a-z0-9_-]`, unique, immutable identifier.
  - `model_prefix`: String prefixed to model names (e.g. `codex/` or `agy-`).
  - `type`: `openai_oauth`, `antigravity_oauth`, or `openai_compatible`.
  - `encrypted_credentials`: AES-256-GCM ciphertext of JSON payload (`{accessToken, refreshToken, accountId/projectId}` or `{apiKey}`).
  - `expires_at`: Unix timestamp when OAuth access token expires.
  - `refresh_started_at`: Lease timestamp to prevent concurrent token refresh races.
- **Model**:
  - Identified by `(provider_id, upstream_name)`.
  - `name`: Public routing ID (`model_prefix + (name_override ?? upstream_name)`).
  - `enabled`: Effective status is `enabled_override ?? provider.enabled`.

---

## 4. Security & Authentication

1. **Bearer Token Authentication**:
   - `/panel-api/*`: Requires `Authorization: Bearer <panel_token>` matching `PANEL_TOKEN_HASH`.
   - `/api/*`: Requires `Authorization: Bearer <gateway_key>` matching `API_KEY_HASH`.
   - Token digests are compared using constant-time comparison. Mismatch yields HTTP 401.
2. **Credential Encryption**:
   - Stored secrets are encrypted at rest using AES-256-GCM with a 32-byte key (`CREDENTIALS_AES_KEY`).
   - Format: `IV (12B) || Ciphertext || Tag (16B)`.
3. **Network Isolation**:
   - Outbound requests support routing through an egress proxy (`ALL_PROXY`, `HTTP_PROXY`, or configuration).
   - Inbound endpoints do not use sessions, cookies, or CORS headers.

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
- **Egress Proxy**: Outbound calls must route through an IP not blocked by Cloudflare WAF / Bot Management.

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
    "model": "<runtimeModel>",
    "request": <GenerateContentRequest>,
    "requestType": "agent",
    "userAgent": "antigravity",
    "requestId": "agent-<uuid>"
  }
  ```
- **Model Mapping**: `gemini-3.7-flash` maps to `gemini-3.7-flash-tiered` with `thinkingLevel` (`LOW`, `MEDIUM`, `HIGH`). `thoughtSignature` blobs are preserved and reattached across turns.

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

## 7. Panel API (`/panel-api/*`)

All panel routes require Bearer token authorization.

| Method | Path | Description |
| :--- | :--- | :--- |
| `GET` | `/panel-api/list-providers` | List all configured providers and their status |
| `POST` | `/panel-api/save-provider` | Create or update a provider |
| `POST` | `/panel-api/delete-provider` | Delete a provider and its models |
| `POST` | `/panel-api/start-provider-oauth` | Generate OAuth authorize URL and PKCE state |
| `POST` | `/panel-api/complete-provider-oauth` | Exchange auth code for tokens and save provider |
| `POST` | `/panel-api/discover-models` | Refresh available models from upstream |
| `GET` | `/panel-api/list-models` | List all routed models and their overrides |
| `POST` | `/panel-api/save-model` | Update model overrides (name, enabled, thinking levels) |

---

## 8. Configuration & Deployment

### Environment Variables

| Variable | Default | Description |
| :--- | :--- | :--- |
| `JANUSGATE_HOST` | `0.0.0.0` | Listen host |
| `JANUSGATE_PORT` | `8080` | Listen port |
| `JANUSGATE_DB_PATH` | `janusgate.db` | Path to in-process Turso SQLite database file |
| `JANUSGATE_API_KEY_HASH` | *(required)* | Hex-encoded SHA-256 digest of gateway Bearer token |
| `JANUSGATE_PANEL_TOKEN_HASH` | *(required)* | Hex-encoded SHA-256 digest of panel Bearer token |
| `JANUSGATE_CREDENTIALS_AES_KEY` | *(required)* | Base64-encoded 32-byte AES-GCM key |
| `ALL_PROXY` / `HTTP_PROXY` | `None` | Optional upstream HTTP/SOCKS5 proxy URL |

### Running the Server
```bash
cargo build --release
./target/release/janusgate
```
