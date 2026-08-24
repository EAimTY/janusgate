# JanusGate — Codex-Compatible Gateway · MVP Design

> Single source of truth for this project. Contracts, invariants, and assumptions stated here are **normative**.

---

## 1. Overview & Scope

**JanusGate** is a minimal Cloudflare-native gateway exposing an OpenAI-compatible API (`/api/openai-v1`) backed by heterogeneous upstream accounts, plus a token-protected admin panel (`/panel-api/*` + SPA).

**Provider types** (three peers; no hierarchy):

| Type | Identity | Upstream wire format |
| --- | --- | --- |
| `openai_oauth` | ChatGPT account | OpenAI Response (`openai_response`), native passthrough |
| `antigravity_oauth` | Google account | Gemini GenerateContent (`gemini_generate_content`) via Google Code Assist |
| `openai_compatible` | Static API key | `openai_response`, `openai_chat_completion`, or `gemini_generate_content` |

**Outward protocols:** OpenAI Response and Chat Completions APIs, each in streaming (SSE) and non-streaming mode. The client's `stream` flag is relayed upstream as-is.

**Wire formats:** every upstream speaks exactly one of three named formats — `openai_response`, `openai_chat_completion`, or `gemini_generate_content`. OAuth kinds imply their format (`openai_oauth` → `openai_response`; `antigravity_oauth` → `gemini_generate_content`); API-key providers choose one explicitly.

**Goals:** stateless per-request processing · zero persistence of prompts/responses/usage · minimal moving parts · single-admin operation.

**Excluded:** pooling, failover, load balancing, automatic/manual provider switching (incl. among `openai_oauth` accounts) · provider types beyond the three above · server-side web-search/shell tool emulation · Anthropic API / Claude Code support in either direction · request logs, usage tracking, metrics · multiple gateway keys, key metadata, rate limiting, retries · WebSockets, Realtime/voice APIs, Durable Objects · multi-tenancy, users/roles/sessions, AES key rotation.

---

## 2. Technology & Deployment

| Layer | Choice |
| --- | --- |
| Backend | Cloudflare Workers, Hono, TypeScript, Cloudflare D1 |
| Frontend | SolidJS v2, Tailwind CSS, Vite, TypeScript |
| Assets | Worker Static Assets |
| AuthN | Shared-token headers (panel + gateway); no sessions |

Deployed as a single Worker + Static Assets. Custom domain **not** required (`*.workers.dev` acceptable).

```jsonc
// wrangler.jsonc
"assets": {
  "directory": "./dist",
  "not_found_handling": "single-page-application",
  "run_worker_first": ["/api/*", "/panel-api/*"]
}
```

`run_worker_first` guarantees API prefixes always reach the Worker, never shadowed by static assets. All other paths serve the SPA; unknown paths return `index.html`.

| Route group | Auth | Handler |
| --- | --- | --- |
| `/panel-api/*` | Panel token + CSRF rule (§4) | Panel JSON API |
| `/api/*` | Gateway key (§4) | Public API |
| everything else | none | Static SPA assets |

No CORS headers are emitted (panel is same-origin only). No health endpoint. No CSP headers (first-party-only SPA, no third-party assets; residual XSS risk accepted for single-admin operation).

---

## 3. Core Concepts

### 3.1 Provider

One configured upstream account or service (types in §1). Fields shared by all types:

- `id` — auto-increasing integer PK; purely internal FK target; **never exposed by any API**.
- `name` — the external string identifier: 1–64 chars `[a-z0-9_-]`, unique, **immutable** after creation. Chosen at creation; OAuth flows accept it as a parameter or auto-generate `openai-<n>` / `antigravity-<n>` with the smallest free `n`. Addresses the provider everywhere (panel routes, prefix seeding); not exposed by the gateway API.
- `modelPrefix` — seeds model-name composition (§6.1):
  - omitted at creation ⇒ defaults to `name`; `""` ⇒ no prefix; non-empty ⇒ used verbatim (≤ 32 chars, charset `[a-z0-9._:/-]`; include separators like `/` or `:` yourself — concatenation inserts none).
  - Omitted on update ⇒ unchanged. Evaluated only at model creation/discovery/save time; changing it never renames existing models. Prefixes are **not unique** across providers; resulting name collisions surface as ordinary uniqueness errors at write time (§6.1).

Per-type stored config and credentials payloads:

| Type | Stored config | Credentials payload (encrypted, §4.3) |
| --- | --- | --- |
| `openai_oauth` | `enabled`, credentials, token `expires_at` | `{ "accessToken", "refreshToken", "accountId" }` — `accountId` parsed from the `chatgpt_account_id` claim of the OAuth `id_token` (default workspace; workspace selection unsupported) |
| `antigravity_oauth` | `enabled`, credentials, token `expires_at` | `{ "accessToken", "refreshToken", "projectId" }` — `projectId` resolved once during onboarding (§8.4), stored with tokens |
| `openai_compatible` | `enabled`, `baseUrl` (valid http(s) URL, canonicalized to **no** trailing slash), `protocol` ∈ {`openai_response`, `openai_chat_completion`, `gemini_generate_content`}, credentials | `{ "apiKey" }` — mandatory at creation; optional on update (omitted ⇒ preserve existing) |

A stored `baseUrl` never ends with a slash (stripped at save); path segments are joined with an explicit `/`: `{baseUrl}/models`, `{baseUrl}/responses`, `{baseUrl}/chat/completions`, and for `gemini_generate_content` `{baseUrl}/models/{model}:generateContent` / `{baseUrl}/models/{model}:streamGenerateContent?alt=sse`. Authentication is `Authorization: Bearer <apiKey>` for the two OpenAI formats and `x-goog-api-key: <apiKey>` for `gemini_generate_content`. A Gemini `baseUrl` includes its version path (e.g. `https://generativelanguage.googleapis.com/v1beta`).

### 3.2 Model

A public model ID mapping to **exactly one provider — any of the three types, `openai_oauth` included** — plus an upstream model ID. No pool, no default routing: a model without a concrete provider cannot exist. Optional `thinkingLevels` override (§10.2). See §6 for naming/serving rules.

---

## 4. Security

### 4.1 Authentication

- Panel (`/panel-api/*`) and gateway (`/api/*`) both require `Authorization: Bearer <token>`. Secrets `PANEL_TOKEN_HASH` / `API_KEY_HASH` store hex-encoded SHA-256 digests; the presented token is hashed and compared **constant-time** (`crypto.subtle.timingSafeEqual` — a Workers-specific `SubtleCrypto` extension, not part of standard WebCrypto). Mismatch → `401`.
- Single gateway key; recommended format `gw_<base64url(32 random bytes)>`. Rotation = setting the new secret; the old key dies immediately. No key metadata, listing, or multi-key support.
- **No sessions, no cookies** — none are ever set, sent, or read anywhere.
- SPA storage: panel token lives in `localStorage` (key from `panel/config.ts`, default `panel_token`); the SPA attaches the header itself. A **non-dismissable full-screen overlay** prompts for the token on load and on any `401`; it verifies via `GET /panel-api/list-providers`. A "forget token" action clears storage and re-shows the overlay.

### 4.2 CSRF

Authentication travels exclusively in a caller-supplied `Authorization` header (no cookies for a browser to attach autonomously), so classic CSRF is structurally impossible. Mandatory defense-in-depth on every panel `POST`: `Origin` must be present and equal the request's own origin, else `403`. `Content-Type` is not a security control (malformed bodies just fail JSON parsing → `400`). Programmatic panel clients must send a correct `Origin`.

### 4.3 Credential encryption at rest

- AES-256-GCM via WebCrypto; key = secret `CREDENTIALS_AES_KEY` (32 bytes, base64).
- Per value: random 12-byte IV; the JSON credentials payload encrypted and stored as one BLOB laid out `IV ‖ ciphertext ‖ tag`.
- The key must never change — no rotation exists; replacing it makes all stored credentials unreadable.
- Plaintext credentials live only in Worker memory for one request's duration. Encrypted values are never returned by any API.
- All timestamps (`*_at`) everywhere are **Unix seconds**.

---

## 5. Provider Resolution

### 5.1 Uniform request-time resolution (all types)

There is no pooling/failover/switching; every model binds to exactly one provider.

1. Model lookup yields its provider (existence FK-guaranteed).
2. `model.enabled ∧ provider.enabled`, else `404 model_not_found` — disabling is externally indistinguishable from nonexistence.
3. OAuth kinds: ensure fresh access token (§7).
4. Dispatch:

| Provider kind | OpenAI Response client | Chat Completions client |
| --- | --- | --- |
| `openai_oauth` | Native response passthrough | Translate through IR (§9) |
| `openai_compatible` · `openai_response` | Native response passthrough | Translate through IR (§9) |
| `openai_compatible` · `openai_chat_completion` | Translate through IR (§9) | Native response passthrough |
| `openai_compatible` · `gemini_generate_content` | Translate through IR (§9) | Translate through IR (§9) |
| `antigravity_oauth` | Translate through IR (§9) | Translate through IR (§9) |

Quota/account errors surface as `upstream_error` (§11.2).

### 5.2 Deletion

Deleting a provider referenced by any model fails via FK `RESTRICT`; the panel surfaces the error.

---

## 6. Models

Fields: public `name` (globally unique), `provider_id` (NOT NULL FK), `upstream_name`, `enabled`, optional `thinkingLevels`.

### 6.1 Naming (normative)

`effectivePrefix(provider)` = the stored `model_prefix` verbatim (`""` ⇒ nothing), seeded with the provider's `name` at creation.

- **Creation/discovery:** `name = effectivePrefix + upstream_name` (the discovered ID on discovery runs), computed once and stored. Clients do not choose it.
- **Rename:** a supplied new *base* recomposes the name as `effectivePrefix + base`; the prefix always survives a rename. A rename is **destructive**: the previous name stops resolving immediately and is freed for reuse — no alias, shadow route, or history remains. Upstream routing (`provider_id`, `upstream_name`) is untouched.
- **Duplicates:** any write producing an existing `name` fails with a `409`-style error. The gateway never auto-renames or suffixes to resolve collisions.
- Nothing recomposes names at request time; the public identity is exactly `models.name`.

### 6.2 Serving

Resolution is exact, case-sensitive match among enabled models only; miss → `404 model_not_found`. All gating failures (model disabled, provider disabled) produce the identical `404`.

### 6.3 Model discovery

Populates models from a provider's upstream. Triggered automatically whenever a provider is added (the SPA calls `discover-models` right after creation/completed OAuth) and re-runnable manually.

| Provider type | Source |
| --- | --- |
| `openai_compatible` · `openai_response` / `openai_chat_completion` | `GET {baseUrl}/models` with `Authorization: Bearer <apiKey>` → `data[].id` |
| `openai_compatible` · `gemini_generate_content` | `GET {baseUrl}/models` with `x-goog-api-key: <apiKey>` → `models[].name` minus the `models/` prefix, keeping only entries whose `supportedGenerationMethods` includes `generateContent` |
| `openai_oauth` | Pinned Codex catalog in the adapter module |
| `antigravity_oauth` | Pinned Gemini-family catalog in the adapter module |

Rules (normative):

- Discovered rows start **disabled**, `thinkingLevels` unset, referencing their own provider (`openai_oauth` discoveries bind to that provider like any other).
- A computed `name` that already exists is **skipped** (counted in the run result); the same ID twice within one run is an **error** aborting the run with no changes.
- Discovery never overwrites, disables, or deletes existing models.
- A failed run changes nothing; the error returns to the panel.

---

## 7. Token Refresh (race-safe leasing)

Access tokens refresh lazily with margin `TOKEN_REFRESH_MARGIN_SECONDS` (default 300): fresh iff `expires_at − now > margin`. Leasing guarantees **at most one in-flight credential mutation per provider** (OpenAI rotates refresh tokens, so double consumption can invalidate one; Google tolerates reuse — the stricter rule covers both):

```text
ensureFresh(provider):
  row ← SELECT … WHERE id
  if row.expires_at − now > MARGIN: return token(row)

  # acquire lease: atomic compare-and-set; leases expire after LEASE_SECONDS
  changed ← UPDATE providers SET refresh_started_at = :now
            WHERE id = :id
              AND (refresh_started_at IS NULL OR refresh_started_at < :now − LEASE_SECONDS)
  if changed = 0:
      # no waiting exists: another mutation holds the lease
      return token(row)   # current-best token; an upstream 401 enters the forced-refresh path

  # lease owner — re-read first: another worker may have refreshed during the lease race
  row ← SELECT … WHERE id
  if row.expires_at − now > MARGIN:
      clear refresh_started_at (guarded)
      return token(row)

  result ← POST refresh to IdP     # response validated structurally: HTTP 2xx + JSON schema (§13)
  success:
      UPDATE … SET credentials = :blob, expires_at = :new_exp, refresh_started_at = NULL
      WHERE id = :id AND refresh_started_at = :my_lease_ts   # guard; changes=0 → discard result
      return new token
  failure:
      clear refresh_started_at (guarded)
      fail upstream_error          # invalid_grant is permanent: provider stays stored but unusable until reauthentication
```

Additional rules:

- Crash safety comes from lease expiry (`REFRESH_LEASE_SECONDS`, default 60).
- Reauthentication completion (§8.5) takes the same lease — it serializes *all* credential mutations per provider.
- **No waiter path exists.** A lease-busy reader proceeds with the current-best token instead of polling; staleness is recovered by the forced-refresh cycle below. This keeps `503` out of the public API entirely.
- **Forced refresh:** upstream `401` ⇒ one forced refresh (margin ignored, same lease, a **single** compare-and-set attempt — lease held ⇒ the `401` surfaces as `upstream_error`) and exactly one retry; a second `401` becomes `upstream_error`.
- Decrypted tokens may be cached in isolate memory per request only.
- Invariants: no refresh token consumed twice concurrently; no two credential writes overlap; readers observe old-or-new credentials atomically (single-row update).

---

## 8. OAuth Paste-Back Flows

Both OAuth providers register loopback redirect URIs that cannot point at the gateway, so login uses **paste-back**: the admin authorizes in the provider's page, the browser lands on a dead loopback URL, and its full URL is pasted back into the panel.

### 8.1 Common mechanics

- `start-provider-oauth` generates `state` (random, single-use) and PKCE verifier/challenge (S256), stores an `oauth_states` row (TTL 900 s, Unix seconds), returns `{ "authorizeUrl": "…" }`. The row carries only what verification needs (`type`, `state`, PKCE `verifier`, optional reauth target, expiry) — creation parameters are **not** stored. The SPA opens the URL in a new tab from a click handler.
- `complete-provider-oauth` receives the pasted URL (+ creation fields, ignored on reauth), parses `code`/`state`/`error`, validates the state row (exists, unexpired, type-consistent; `error=` instead of `code` → `400`), exchanges the code (token response validated structurally — HTTP status + JSON schema, §13), encrypts credentials, creates or updates the provider (`name` defaults to auto-generation; duplicate → panel error), deletes the state row, returns `{ "ok": true }`.
- Expired state rows are inert (every use re-checks expiry) and are swept by a single opportunistic DELETE inside `complete-provider-oauth`.

### 8.2 Completion atomicity

`complete-provider-oauth` is all-or-nothing. Pipeline: token exchange → project resolution/onboarding (Antigravity only) → credential encryption → (reauth only: acquire the §7 lease) → a **single atomic D1 batch** writing the provider row and deleting the state row together. Any earlier failure deletes the state row, touches no provider (new flow: no row left; reauth: previous credentials stay active), and returns a panel error — the admin restarts the flow from scratch.

### 8.3 OpenAI specifics

Constants pinned in the adapter module (public Codex CLI OAuth client):

- Client ID `app_EmoamVZd6MmPgQohKd7qKLFF` (public PKCE client, no secret).
- Authorize `https://auth.openai.com/authorize`: `response_type=code`, `scope=openid profile email offline_access`, `redirect_uri=http://localhost:1455/auth/callback`, `code_challenge_method=S256`, `state`.
- Token `POST https://auth.openai.com/oauth/token` (form-encoded): grants `authorization_code` / `refresh_token` with `code_verifier` / `refresh_token`, `client_id`, `redirect_uri`. Refresh tokens are reusable; `expires_in` feeds `providers.expires_at`.

Upstream API: `POST https://chatgpt.com/backend-api/codex/responses` — SSE when `stream:true`, JSON object when `stream:false` — with headers:

```text
Authorization: Bearer <accessToken>
chatgpt-account-id: <accountId>
Originator: codex_cli_rs
User-Agent: <pinned Codex CLI UA string>
Session-Id: <uuid, generated per request or forwarded from client>
Accept: text/event-stream (stream:true) / application/json (stream:false)
```

Requests pass through natively as Responses payloads (`store:false` typical); `instructions` forwarded as-is.

### 8.4 Antigravity specifics

Constants pinned in the adapter module (the Antigravity editor's installed-app OAuth client):

- Authorize `https://accounts.google.com/o/oauth2/v2/auth`: `response_type=code`, `scope=https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email`, `access_type=offline`, `prompt=consent`, registered loopback `redirect_uri`, PKCE S256, `state`.
- Token `POST https://oauth2.googleapis.com/token`: `authorization_code` / `refresh_token` grants with client ID **and** client secret + `code_verifier`.
- API base `https://cloudcode-pa.googleapis.com/v1internal`.

**Onboarding (once, after first login):** call `loadCodeAssist`; if unprovisioned, call `onboardUser`. The resolved project ID is stored as `projectId` and sent as `project` in every API call. Reauthentication reuses the stored `projectId` and skips onboarding unless missing.

**Upstream calls:** `POST …/{project}/v1internal:streamGenerateContent?alt=sse` (streaming) or `…/{project}/v1internal:generateContent` (non-streaming), `Authorization: Bearer <accessToken>`. Body: `{ model, project, request: { contents[], systemInstruction, tools{functionDeclarations}, generationConfig } }`. Streaming responses are SSE frames, each a `GenerateContentResponse` JSON; non-streaming returns a single such JSON.

Adapter responsibilities:

- Map outward messages/parts ⇄ `contents[]` roles and parts (`text`, `inlineData` for data-URL images, `functionCall`/`functionResponse`).
- Map sampling: `temperature`, `topP`, `maxOutputTokens`, `stop`, `responseMimeType` (`json_object`/`json_schema`).
- Map `reasoning.effort` → `generationConfig.thinkingConfig` (§10.2); surface `thought:true` parts as reasoning events.
- Synthesize tool-call IDs (`call_N`) for `functionCall` parts (Code Assist has none); match echoed `functionResponse` parts back by index/name.
- **Preserve `thoughtSignature` blobs** wherever present, reattaching them to corresponding parts in follow-up turns within the same request history. Signatures cannot survive across independent client requests (stateless gateway); their absence degrades gracefully — accepted limitation.
- Upstream model IDs are free-form passthrough strings (e.g. `gemini-3-pro-high`).

⚠️ These endpoints/constants are reverse-engineered from the Antigravity client and may change without notice. They are pinned in one adapter module — the fragile boundary of the system.

### 8.5 Reauthentication

`start-provider-oauth` accepts an optional `providerName`; the state row stores it, and completion replaces that provider's credentials (its type must match the flow's type). Parallel flows for one provider are last-write-wins.

Because reauthentication replaces credentials, it participates in the §7 lease: before its credential write, completion makes a **single** compare-and-set attempt; if the lease is held, the flow aborts (state deleted, nothing changed, panel error — admin retries). Holding the lease, it writes new credentials and clears the lease in one guarded update (`refresh_started_at = :lease_ts`; guard failure → panel error, retry). An in-flight refresh and a concurrent reauth are therefore strictly ordered — exactly one writes last, and a refresh can never overwrite freshly reauthenticated credentials. Creation flows skip leasing (no provider row exists yet).

---

## 9. Translation Architecture (IR)

Pairwise converters are avoided; all translated traffic passes through a small internal event IR:

```text
request:  messages(system/user/assistant/tool), parts(text/image/tool_call/tool_output),
          tools(function), sampling, reasoning-effort
events:   message_start · text_delta · tool_call_start/delta/done ·
          reasoning_delta · finish(reason) · usage · done
reasons:  stop | length | tool_calls | content_filter
```

| Component | Direction | Implementations |
| --- | --- | --- |
| Outward parser/renderer | client ⇄ IR | OpenAI Response, Chat Completions |
| Upstream emitter/reader | IR ⇄ upstream | OpenAI Response (`openai_response`), Chat Completions (`openai_chat_completion`), Gemini GenerateContent (`gemini_generate_content`) |

The IR is used **only when the client protocol and upstream protocol differ**, or when a `gemini_generate_content` upstream requires translation. A same-format request — OpenAI Response client to `openai_response` upstream, or Chat Completions client to `openai_chat_completion` upstream — bypasses the response parser and renderer entirely. After request validation, routing, and upstream model substitution, the upstream response body is relayed in its original bytes rather than parsed into objects/events and serialized again. This applies to streaming and non-streaming successes and errors.

Consequently, a same-format response retains the upstream's IDs, `model`, usage fields, event sequence, JSON formatting, and any unknown fields exactly as returned. The gateway forwards `stream_options` but does not rewrite `chunk.model` or synthesize an `include_usage` chunk on this path. Those normalizations apply only when the response is translated through the IR.

Usage and finish reasons map across translated representations (`prompt_tokens ⇄ input_tokens ⇄ promptTokenCount`, etc.; `SAFETY → content_filter`). A translated non-streaming response parses the complete upstream object into IR events and renders the outward object from that event sequence.

---

## 10. Feature Rules

### 10.1 Matrix

| Feature | Same-format native passthrough | Translated paths |
| --- | --- | --- |
| `tools` of type `function` | passthrough | ✔ |
| Other tool types (`web_search`, …) | passthrough | `400 unsupported_tool_type` |
| `previous_response_id`, `store:true` | passthrough — effective only against upstreams that keep server-side conversation state; stateless upstreams reject it and the upstream error surfaces (§11.2) | `400 unsupported_parameter` |
| Reasoning | native fields/items/events | Responses out: reasoning deltas when upstream provides them; CC out: best-effort `delta.reasoning_content`, dropped if absent |
| Image inputs | passthrough | passthrough (CC); data-URL only on the Gemini path, else `400` |
| `n` > 1 (Chat Completions inbound) | `400 unsupported_parameter` | `400 unsupported_parameter` |
| Unknown top-level fields | passthrough | ignored |
| `instructions` | passthrough | → system message / `systemInstruction` |

### 10.2 Reasoning effort

Reasoning effort is an **opaque non-empty string**, not an enum. The gateway validates only its shape and otherwise passes it through as far as the wire formats allow; there is no canonical value set enforced on client requests.

- Inbound effort is read from Responses `reasoning.effort` or CC `reasoning_effort`. Non-string or empty values → `400 unsupported_parameter`; any other string is accepted.
- **Same-format requests** relay the client's value verbatim inside the substituted body; cross-format OpenAI⇄OpenAI translation copies it verbatim into the target field.
- **Per-model whitelist:** a model's `thinkingLevels` override (panel vocabulary ⊆ `minimal < low < medium < high < xhigh`) acts as a plain whitelist: requested effort ∈ override → forwarded verbatim; otherwise the reasoning parameter is omitted entirely. No nearest-lower clamping exists — arbitrary strings have no order to clamp along.
- **Gemini upstreams (`gemini_generate_content`) can only express `low|medium|high`:** those strings pass through verbatim; `minimal` omits the config, `xhigh` clamps down to `high`, and any other string means the upstream default applies rather than a guessed mapping (mapping pinned in the Gemini adapter module).
- An omitted client effort ⇒ no reasoning configuration is ever injected upstream.
- **Stripping is not disabling.** A stripped effort means "upstream default applies" — for Gemini models that reason by default, the effective reasoning can therefore *exceed* what the client asked for. `[]` means "never inject", never "off".
- Panel semantics: `thinkingLevels` omitted/`null` ⇒ no filtering (pure passthrough); `[]` ⇒ reasoning always stripped (still not "off"; §14 mirrors this).
- With no override set there are **no format defaults**: every OpenAI-format effort reaches the upstream unchanged, and the upstream owns rejecting values it cannot handle (surfacing as `upstream_error`, §11.2).

---

## 11. Public API — `/api/openai-v1`

### 11.1 Routes

| Method | Route | Purpose |
| --- | --- | --- |
| GET | `/api/openai-v1/models` | List enabled models |
| POST | `/api/openai-v1/responses` | Responses-API chat (streaming + non-streaming) |
| POST | `/api/openai-v1/chat/completions` | Chat Completions chat (streaming + non-streaming) |

### 11.2 Conventions

**Lifecycle (both POST routes):** authenticate → parse/validate → resolve model → resolve provider (§5.1) → ensure fresh token (OAuth kinds, §7) → build upstream request (`stream` forwarded as-is) → relay the upstream response directly when its protocol matches the client protocol, otherwise translate it through the IR.

**Invariant — same-format response passthrough:** `openai_response`→`openai_response` and `openai_chat_completion`→`openai_chat_completion` responses bypass JSON/SSE parsing and serialization. The gateway preserves the upstream status, safe headers, and exact body bytes in both streaming and non-streaming modes. Raw non-streaming bytes may be buffered solely to enforce `MAX_RESPONSE_BYTES`; they are never decoded or re-encoded. Request parsing, validation, model resolution, model substitution, and reasoning-effort enforcement still occur before the upstream call.

**Invariant — stream relay:** the client's `stream` value reaches the upstream unchanged (protocol-equivalent form, e.g. Gemini `:streamGenerateContent?alt=sse` vs `:generateContent`) and the upstream's mode reaches the client unchanged. The gateway never converts between streaming and non-streaming modes, never buffers a stream into a non-streaming object, and never synthesizes error frames or response objects.

**Limits:** request bodies ≤ `MAX_REQUEST_BYTES` (10 MB). Enforcement is byte-counted and never trusts `Content-Length` alone: a present `Content-Length` above the limit rejects immediately with `413` before reading; the body is then buffered through a counting stream, and crossing the limit mid-read aborts with `413 payload_too_large`. Non-streaming upstream bodies read up to `MAX_RESPONSE_BYTES` (25 MB) — overflow aborts the upstream fetch → `502 upstream_response_too_large`.

**Streaming failures:**
- *Before the first byte reaches the client:* any failure (auth, resolution, upstream non-2xx, first-byte idle timeout) yields an HTTP error per the error-transparency policy below. Nothing partial is sent.
- *After streaming began:* the gateway never synthesizes terminal frames. Upstream disconnect, malformed stream, or idle timeout → abort the upstream fetch and simply end the client stream. A valid stream always ends with `response.completed` (Responses) or `data: [DONE]` (CC); ending any other way **is** the client-visible error signal.
- Idle timeout: no upstream bytes for `UPSTREAM_IDLE_TIMEOUT_SECONDS` (300) → `504` if nothing was sent yet, otherwise the stream just ends.
- Client disconnect: `request.signal` aborts the upstream fetch via `AbortController` (also during non-streaming reads).

**Non-streaming failures:** per the error-transparency policy below. On translated paths, unparseable 2xx JSON → `502 upstream_failure`. Same-format paths never parse the response body. Idle timeout → `504 upstream_timeout`.

**Error envelope (all errors):**

```json
{ "error": { "message": "Human-readable explanation", "type": "invalid_request_error", "code": "optional_machine_code" } }
```

| Status | `type` | Typical `code` |
| --- | --- | --- |
| 400 | `invalid_request_error` | `invalid_json`, `unsupported_parameter`, `unsupported_tool_type`, `payload_too_large` |
| 401 | `authentication_error` | `invalid_api_key` |
| 404 | `not_found_error` | `model_not_found` |
| 413 | `invalid_request_error` | `payload_too_large` |
| 429 | `rate_limit_error` | *(upstream)* |
| 500 | `api_error` | `internal_error` |
| 502 | `upstream_error` | `upstream_failure`, `upstream_auth_rejected`, `upstream_response_too_large` |
| 504 | `upstream_error` | `upstream_timeout` |

**Error transparency (normative).** The gateway rewrites only what it must:

- **Native passthrough paths** — no protocol change and no streaming-mode change (`openai_response` upstream serving OpenAI Response clients, or `openai_chat_completion` upstream serving Chat Completions clients): the entire upstream response, success *or* error, bypasses parsing and re-serialization. Its status, safe headers (`Content-Type`, `Retry-After`, …), and exact body bytes are relayed unchanged. Transparent pipe; no sanitization.
- **Translated paths** — the gateway authors the envelope: upstream `429` → `429` with the upstream `Retry-After` forwarded; every other upstream failure → `502 upstream_error`. Bodies: `openai_compatible` upstreams relay their OpenAI-shaped error object **verbatim** (admin-chosen, semi-trusted upstream — its messages are operationally valuable); OAuth kinds get **sanitized** messages — never proxied verbatim, no tokens, URLs, or internal identifiers.

### 11.3 `GET /api/openai-v1/models`

Returns enabled models sorted by internal `id`. `owned_by`: `openai_oauth` → `"openai"`, `antigravity_oauth` → `"antigravity"`, `openai_compatible` → `"custom"`.

```json
{
  "object": "list",
  "data": [
    { "id": "gemini-3-pro", "object": "model", "created": 0, "owned_by": "antigravity" },
    { "id": "gpt-codex",    "object": "model", "created": 0, "owned_by": "openai" },
    { "id": "oss-120b",     "object": "model", "created": 0, "owned_by": "custom" }
  ]
}
```

### 11.4 `POST /api/openai-v1/responses`

Accepts a standard OpenAI **Responses** request; `model` must resolve (§6.2); `stream` defaults to `false` and is forwarded as-is.

- `stream:true` → SSE relay: exact-byte passthrough from a Responses upstream, or IR translation from another upstream protocol.
- `stream:false` → exact response-body bytes from a Responses upstream without JSON parsing/re-serialization, or object→IR→object translation from another upstream protocol.

The gateway never buffers a stream to synthesize a non-streaming object.

### 11.5 `POST /api/openai-v1/chat/completions`

Accepts a standard OpenAI **Chat Completions** request; same lifecycle.

- `stream:true` → exact-byte SSE passthrough from a Chat Completions upstream, or IR-rendered chunks ending with `data: [DONE]` from another upstream protocol. On translated paths, `stream_options.include_usage` is enforced with a usage-only chunk before `[DONE]`; on native paths it is forwarded and the upstream response is left unchanged.
- `stream:false` → exact response-body bytes from a Chat Completions upstream without JSON parsing/re-serialization, or object→IR→object translation from another upstream protocol.
- `n` > 1 → `400 unsupported_parameter` (single-choice generation only).

---

## 12. Panel API — `/panel-api`

### 12.1 Routes

| Method | Route | Purpose |
| --- | --- | --- |
| GET | `/panel-api/list-providers` | All providers (doubles as auth probe) |
| POST | `/panel-api/save-provider` | Create/update provider |
| POST | `/panel-api/delete-provider` | Delete provider (FK-guarded) |
| POST | `/panel-api/start-provider-oauth` | Begin paste-back OAuth |
| POST | `/panel-api/complete-provider-oauth` | Finish paste-back OAuth |
| GET | `/panel-api/list-models` | All model mappings |
| POST | `/panel-api/save-model` | Create/update model |
| POST | `/panel-api/delete-model` | Delete model |
| POST | `/panel-api/discover-models` | Run discovery for one provider |

### 12.2 Conventions

- Auth + CSRF per §4.1–4.2 (every `POST` requires matching `Origin`).
- Responses are JSON: successes `{"ok": true}` or a resource payload; errors `{"error": "…"}` with 4xx/5xx. `POST` bodies are JSON objects.
- Providers are addressed exclusively by their `name` in panel requests (numeric `providers.id` never appears); models are addressed by numeric `id`.
- Validation (application-side, before write): provider `name` pattern/uniqueness/immutability, `modelPrefix` rules (§3.1), composed model-`name` uniqueness (§6.1 — duplicates fail outright), `thinkingLevels` ⊆ canonical vocabulary (§10.2), `baseUrl` http(s) stripped of any trailing slash, `protocol` enum, booleans as 0/1.

### 12.3 `GET /panel-api/list-providers`

Returns all providers, never credentials. Doubles as the SPA auth-probe endpoint.

```json
{
  "providers": [
    { "name": "work", "type": "openai_oauth", "protocol": null,
      "baseUrl": null, "expiresAt": 1785000000, "enabled": true,
      "modelPrefix": "" },
    { "name": "antigravity-1", "type": "antigravity_oauth", "protocol": null,
      "baseUrl": null, "expiresAt": 1785000000, "enabled": true,
      "modelPrefix": "gemini/" },
    { "name": "openrouter", "type": "openai_compatible", "protocol": "openai_chat_completion",
      "baseUrl": "https://openrouter.ai/api/v1", "expiresAt": null, "enabled": true,
      "modelPrefix": "openrouter/" }
  ]
}
```

### 12.4 `POST /panel-api/save-provider`

- **Create:** only `type: "openai_compatible"`; requires `name`, `protocol`, `baseUrl`, `apiKey`; optional `enabled`, `modelPrefix`.
- **Update:** `name` addresses the target. `type` and `name` immutable. `openai_compatible`: `protocol`/`baseUrl`/`enabled`/`modelPrefix`, plus optional `apiKey` (omitted ⇒ preserve existing). OAuth providers: `enabled`/`modelPrefix` only.

Request (update): `{ "name": "openrouter", "enabled": true, "modelPrefix": "or/", "baseUrl": "…", "protocol": "openai_chat_completion", "apiKey": "…" }`
Response: `{ "name": "openrouter" }`

### 12.5 `POST /panel-api/delete-provider`

Request: `{ "providerName": "…" }` → delete by `name`. Referenced-by-a-model deletions fail via FK `RESTRICT` → `409`-style error.
Response: `{ "ok": true }`

### 12.6 `POST /panel-api/start-provider-oauth`

Request: `{ "type": "openai" | "antigravity", "providerName"? }`. With `providerName`: reauthentication flow (provider must exist and match the flow type). Without: creation flow — creation details are supplied at completion (§12.7). Generates state + PKCE (§8.1).
Response: `{ "authorizeUrl": "https://…" }`

### 12.7 `POST /panel-api/complete-provider-oauth`

Request: `{ "redirectUrl": "<full pasted URL>", "name"?, "modelPrefix"? }` — `name`/`modelPrefix` apply to creation flows only, ignored on reauthentication. `error=` param instead of `code` → `400`.

Workflow: validate state (§8.1) → exchange code (`code_verifier`; Antigravity also sends client secret) → Antigravity onboarding where applicable (§8.4) → encrypt credentials → atomic write + state deletion (§8.2; reauth holds the §7 lease across the write, §8.5).
Response: `{ "ok": true }`

### 12.8 `GET /panel-api/list-models`

Model→provider links exposed as `providerName`/`providerType`; storage keeps the integer FK.

```json
{
  "models": [
    { "id": 1, "name": "gpt-codex", "providerName": "work", "providerType": "openai_oauth",
      "upstreamName": "gpt-codex", "enabled": true, "thinkingLevels": null },
    { "id": 2, "name": "gemini/pro", "providerName": "antigravity-1", "providerType": "antigravity_oauth",
      "upstreamName": "gemini-3-pro-high", "enabled": true, "thinkingLevels": ["low", "high"] }
  ]
}
```

### 12.9 `POST /panel-api/save-model`

Request: `{ "id"?, "providerName", "upstreamName", "name"?, "enabled"?, "thinkingLevels"? }`

- `thinkingLevels`: array ⊆ canonical vocabulary (§10.2); omitted/`null` ⇒ no filtering (pure passthrough); `[]` ⇒ reasoning always stripped — suppressing gateway-injected reasoning config, never upstream-default reasoning (§10.2).
- `name`, when present, is the **base** — stored public name becomes `effectivePrefix(provider) + name` (§6.1); omitted ⇒ base is `upstreamName`. Supplying `name` on an update is a destructive rename (old name stops resolving immediately).
- Validation: `providerName` references an existing provider of **any** type (`openai_oauth` included); non-empty `upstreamName`; `name` when present non-empty; composed name globally unique — duplicates → `409`-style error, never auto-renamed/suffixed.

Response: `{ "id": N, "name": "<full stored name>" }`

### 12.10 `POST /panel-api/delete-model`

Request: `{ "id": N }` → Response: `{ "ok": true }`

### 12.11 `POST /panel-api/discover-models`

Request: `{ "providerName": "…" }` → runs the discovery strategy (§6.3).
Response: `{ "total": 12, "created": 9, "skipped": 3 }`
Upstream failures surface as a panel error with no state change. Invoked automatically right after provider creation/OAuth; re-running is always safe (idempotent skips).

---

## 13. D1 Schema

Single migration. All tables `STRICT`; timestamps Unix seconds. Deliberately **no `CHECK` constraints** — the database is infallible storage, not a validator (policy below).

```sql
CREATE TABLE providers (
    -- auto-increasing surrogate; internal only, never exposed by any API
    id INTEGER PRIMARY KEY,

    -- string identifier; displayed as-is; doubles as default model-name prefix (§3.1, §6.1)
    name TEXT NOT NULL UNIQUE,

    -- seeded with the provider name at creation; '' ⇒ no prefix; else used verbatim (§3.1)
    model_prefix TEXT NOT NULL DEFAULT '',

    type TEXT NOT NULL,

    protocol TEXT,

    base_url TEXT,

    -- AES-256-GCM blob: IV ‖ ciphertext ‖ tag (§4.3)
    encrypted_credentials BLOB NOT NULL,

    expires_at INTEGER,

    refresh_started_at INTEGER,

    enabled INTEGER NOT NULL DEFAULT 1
) STRICT;


CREATE TABLE models (
    id INTEGER PRIMARY KEY,

    name TEXT NOT NULL UNIQUE,

    provider_id INTEGER NOT NULL REFERENCES providers(id) ON DELETE RESTRICT,

    upstream_name TEXT NOT NULL,

    -- JSON array ⊆ canonical thinking-level vocabulary; NULL ⇒ no filtering, pure passthrough (§10.2)
    thinking_levels TEXT,

    enabled INTEGER NOT NULL DEFAULT 1
) STRICT;

-- every model binds to exactly one provider (any type, openai_oauth included)
CREATE INDEX idx_models_provider_id ON models(provider_id);


CREATE TABLE oauth_states (
    id INTEGER PRIMARY KEY,

    type TEXT NOT NULL,

    state TEXT NOT NULL UNIQUE,

    verifier TEXT NOT NULL,

    -- reauthentication target (NULL for new-provider flows)
    provider_id INTEGER REFERENCES providers(id) ON DELETE CASCADE,

    expires_at INTEGER NOT NULL
) STRICT;

CREATE INDEX idx_oauth_states_expires_at ON oauth_states(expires_at);
```

Validation policy (application-side, beyond the schema):

- **Human input only gets validated.** Panel API bodies are validated before write (§12.2 list). Validation constants live in the config modules (§15).
- **OAuth/IdP responses are validated structurally; contents stored as-is.** Every token-exchange/refresh/onboarding response must be HTTP 2xx and match the JSON schema implied by §3.1 (fields present with correct JSON types). Field **content** is never validated — no token-format, expiry-plausibility, or issuer checks; shape-valid values are stored as-is. Semantic misbehavior surfaces later as `upstream_error` (§11.2), never as a local rejection. Non-OAuth upstream returns (discovered model IDs, `upstream_name`) remain pure passthrough.
- **Structural guarantees stay in the schema:** `UNIQUE` identities, FKs with `RESTRICT`/`CASCADE`, `NOT NULL`/`DEFAULT`, `STRICT` typing.
- **Lifecycle:** OAuth state rows are single-use, TTL-bounded, deleted on use, swept when expired; `providers.expires_at` is the sole token-expiry authority.

---

## 14. Frontend Panel

SolidJS v2 SPA; client-side routing; native `fetch`; no state library.

- Global **auth overlay** (§4.1): non-dismissable until the probe succeeds.
- `/providers` — table grouped by type, showing `name` as-is; actions: add API-key provider (form), add/reauthenticate OpenAI & Antigravity accounts (paste-back dialog collects the redirect URL plus, for creations, optional `name` and `modelPrefix`), edit, enable/disable, discover models, delete. Shows token `expiresAt` with expired/expiring-soon indication.
- `/models` — mapping table + one create/edit form: provider selector (all providers regardless of type), upstream-name field, optional **base name** field with live preview of the final public name (prefix + base, §6.1), enable checkbox, thinking-level multi-select (blank = passthrough — labeled “no filter, not off”, §10.2); delete.
- `/` redirects to `/providers`; unknown paths render via SPA fallback.
- All SPA tunables come from `panel/config.ts` (§15). Panel calls address providers exclusively by `name` (§12.2).

---

## 15. Configuration & Secrets

Two kinds, strictly apart. Upstream protocol constants are deliberately exempt from config modules — they stay pinned inside their adapter modules (§8.3–8.4), the fragile boundary of the system.

### 15.1 Secrets (Wrangler)

```bash
openssl rand -base64 32                                # plaintext panel token / gateway key (keep!)
echo -n '<plaintext>' | openssl dgst -sha256 -hex       # → PANEL_TOKEN_HASH / API_KEY_HASH
openssl rand -base64 32                                 # → CREDENTIALS_AES_KEY (base64 of 32 random bytes)
wrangler secret put PANEL_TOKEN_HASH && wrangler secret put API_KEY_HASH && wrangler secret put CREDENTIALS_AES_KEY
```

| Name | Value |
| --- | --- |
| `PANEL_TOKEN_HASH` | hex SHA-256 of panel token |
| `API_KEY_HASH` | hex SHA-256 of gateway key (`gw_…`) |
| `CREDENTIALS_AES_KEY` | base64 of 32 random bytes |

Secrets never appear in code, config modules, or the repo. Changing `CREDENTIALS_AES_KEY` destroys all stored credentials (§4.3).

### 15.2 Config modules (everything else)

Every non-secret tunable lives in one of four modules — its single home. **No runtime env vars**; changing a value means editing a module and redeploying (accepted for single-admin operation). No magic numbers elsewhere.

| Module | Scope | Owns (defaults shown) |
| --- | --- | --- |
| `config.ts` | global | Canonical thinking-level vocabulary & order — panel override whitelist only, never a request-time enum (§10.2); provider `name` pattern `[a-z0-9_-]{1,64}`; `modelPrefix` rules (≤ 32 chars, `[a-z0-9._:/-]`); Unix-second convention |
| `api/config.ts` | public API | Base path `/api/openai-v1`; `MAX_REQUEST_BYTES` 10 MB; `MAX_RESPONSE_BYTES` 25 MB; `UPSTREAM_IDLE_TIMEOUT_SECONDS` 300; `TOKEN_REFRESH_MARGIN_SECONDS` 300; `REFRESH_LEASE_SECONDS` 60; forced-refresh retry count 1; `owned_by` mapping (§11.3) |
| `panel-api/config.ts` | panel API | Base path `/panel-api`; `OAUTH_STATE_TTL_SECONDS` 900; auto-generated name prefixes `openai-` / `antigravity-` |
| `panel/config.ts` | SPA | Token localStorage key (`panel_token`); auth-probe endpoint (`/panel-api/list-providers`); SPA routes; token expiry-warning horizon |

---

## 16. Invariants & Assumptions

### Invariants (must always hold)

1. Credentials and API keys exist **only** as AES-GCM ciphertext in D1; plaintext never appears in storage, logs, or API responses.
2. At most one in-flight credential mutation per provider — IdP refresh or reauth completion, serialized by the shared lease; refresh tokens never consumed twice concurrently; observers see old-or-new credentials atomically.
3. Every upstream call carries the best available access token — margin-fresh, or current-best when a concurrent credential mutation holds the lease; a rejected token triggers exactly one forced refresh + retry (§7), whose exhaustion fails with a defined `upstream_error`.
4. Every model binds to exactly one provider (any type, `openai_oauth` included); no pool, no default route. A model serves iff `model.enabled ∧ provider.enabled`, else identical `404 model_not_found` — disabling ≡ nonexistent. Names are globally unique; resolution is exact, case-sensitive match among enabled models.
5. A provider referenced by a model cannot be deleted (`RESTRICT`).
6. Authentication derives exclusively from `Authorization` headers via constant-time digest comparison; the system has no cookies — none set, sent, or read.
7. The client's `stream` value relays to the upstream as-is; the gateway never converts between streaming and non-streaming modes, never synthesizes error frames or response objects. A same-format response bypasses parsing and serialization and reaches the client in the upstream's exact body bytes; only protocol-changing paths use the IR (§9, §11.2). Before streaming begins the client sees that native or translated response, or an error; a mid-stream failure simply ends the stream — the missing terminal event (`response.completed` / `[DONE]`) is the error signal. No hangs.
8. Client disconnects deterministically cancel the upstream request.
9. OAuth states are single-use, expire after `OAUTH_STATE_TTL_SECONDS`, and are deleted on use.
10. Discovery never overwrites, disables, or deletes existing models; discovered rows are created disabled.
11. Client-requested reasoning effort is validated as a non-empty string and filtered against the model's `thinkingLevels` whitelist (when present) before any upstream call; it is never clamped to a value set by the gateway (§10.2).
12. Non-streaming upstream bodies are never read beyond `MAX_RESPONSE_BYTES`; overflow aborts the upstream fetch → `502 upstream_response_too_large`.
13. A provider's `name` is its sole externally visible identifier — unique, immutable, displayed as-is; numeric `providers.id` never leaves storage. Prefixes are not unique. The public model identity is exactly `models.name`; `modelPrefix` affects only models created/discovered afterward; a rename permanently retires the former name (no aliases, no shadow routes).
14. No request, response, error, or usage data is ever persisted.
15. Whenever the client and upstream protocols match — Responses→Responses or Chat Completions→Chat Completions — the response parser and serializer are not invoked. Upstream successes and errors reach the client with exact body bytes and status preserved (§11.2). Only protocol-changing paths carry IR-rendered responses or gateway-authored upstream-error envelopes.

### Known limitations

- **Mid-stream failure ≙ silent EOF (from invariant 7).** A truncated stream is indistinguishable from a clean close for clients that ignore the terminal event; several popular SDKs treat a close without `response.completed` / `[DONE]` as successful completion and may present truncated output as complete. Accepted: completeness-critical clients should validate output or use non-streaming mode.

### Implications (推论结果)

- From (2)+(3): provider lockouts degrade to explicit, observable errors rather than silent token corruption.
- From (7)+(8): the gateway is safe behind impatient clients and long generations alike.

### Assumptions

- Single-admin deployment; panel and gateway keys distributed out-of-band.
- The `chatgpt.com` Codex backend accepts requests from Cloudflare Worker egress (shared datacenter) IPs — the only upstream with meaningful IP-reputation risk; a monitored assumption, not a known blocker.
- Google Code Assist has no known datacenter-IP restrictions (hosting on Workers is not a risk factor for Antigravity); its real risks are undocumented endpoints drifting (§8.4) and potential account-level abuse flags under heavy automated use.
- Host clocks are sane; all timing uses `Date.now()` at the Worker.
