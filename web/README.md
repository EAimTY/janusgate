# janusgate-web — Standalone Web Frontend for JanusGate

> Single source of truth for this project's design and architecture. Contracts and invariants stated here are **normative** for the `web/` workspace.
>
> The JanusGate gateway is normative for everything wire-level. The repository root's `README.md` §7 defines the management API, and the **generated TypeScript bindings (§7) are that contract's machine-readable form** — payload shapes are deliberately not restated in this document, so the two can never drift. When something here disagrees with the gateway, the gateway wins.

---

## 1. Overview & Scope

**janusgate-web** is a standalone, statically-hosted single-page application (SPA) that administers a [JanusGate](https://github.com/EAimTY/janusgate) instance through its versioned management JSON API (`/management-api/v0/*`). It lives in this repository as the `web/` workspace, beside the Rust gateway it administers.

JanusGate itself is **headless**: it exposes an OpenAI-compatible serving API (`/api/openai-v1`) and a management API (`/management-api/v0/*`), and ships no frontend of its own. janusgate-web is the missing control plane UI — deployed **separately, anywhere that serves static files**, talking to any JanusGate instance over the network.

### Deployment model

```text
┌─────────────────────────┐         cross-origin fetch          ┌──────────────────────────┐
│ janusgate-web (static)  │  ─────────────────────────────────▶ │ JanusGate (headless)     │
│ any static host, public │   POST /management-api/v0/<action>  │ anywhere reachable       │
│ no backend of its own   │   Authorization: <token>            │ management_api.token     │
└─────────────────────────┘   Content-Type: application/json    └──────────────────────────┘
```

- **Deploy anywhere.** The build output (`target/web/dist/` from the repository root) is plain static files (`index.html` + JS + CSS). Any static host works; no server-side rendering, no edge functions, no backend of its own.
- **Public by default.** Anyone who can reach the deployed URL can open the app. Opening it grants nothing by itself: every management call requires the target instance's management token, which the app never ships with.
- **First-visit connection setup.** The app holds no connection on first entry, so it presents a connection screen asking for exactly two values: the JanusGate **management API base URL** (e.g. `https://gateway.example.com`) and its **management token** (matching `management_api.token` in that instance's configuration). The pair is verified with one probe call (`POST /management-api/v0/providers:list`) before it is accepted and persisted.
- **Connection persists locally.** The accepted pair lives only in the browser's `localStorage` (key in `src/config.ts`). Returning visits skip setup; a "forget" action clears the stored pair and returns to setup. The app administers one instance at a time.
- **Cross-origin is the normal case.** The app's origin and the gateway's origin are expected to differ, and CORS on the gateway makes that work (§5). No proxying, no same-origin requirement, no backend-for-frontend.

### Goals & non-goals

- **Goals:** one-connection administration of providers, models, serving keys, and OAuth flows; zero build-time or deployment-time configuration — the same artifact points at any instance; minimal moving parts (static files + browser `fetch` + one dev-time codegen step).
- **Excluded:** serving inference traffic (clients call the gateway's `/api/*` directly, never through this app); user accounts, roles, or sessions in the app; usage tracking, metrics, or request logging; multi-tenancy; any credential handling beyond the management token and the serving keys themselves — upstream OAuth secrets and provider API keys are minted inside JanusGate by its own flows, and the app only relays the administrator's input, while serving-key secrets are typed or locally generated in the app and relayed to the gateway.

---

## 2. Technology Stack

| Layer | Choice | Notes |
| :--- | :--- | :--- |
| **Language** | TypeScript (strict) | `tsc --noEmit`, run through the repository root's `npm run check`; strict flags per `tsconfig.json` |
| **UI framework** | SolidJS v2 (`solid-js` + `@solidjs/web`) | Fine-grained reactivity, no virtual DOM; JSX with `jsxImportSource: @solidjs/web` |
| **Styling** | Tailwind CSS v4 (via `@tailwindcss/vite`) | Utility-first over the token set defined in `src/tailwind.css` (`@theme`): light/dark neutrals, one accent, three status hues. `DESIGN.md` documents the world those tokens build |
| **Build / dev server** | Vite | Static output to `target/web/dist/`; the host needs an SPA fallback (§5) |
| **Lint & format** | Biome | `biome.jsonc` at the repository root, covering the whole repository; `npm run check` / `npm run fix` from the root |
| **Tests** | Vitest | `npm test`; TDD per `AGENTS.md` |
| **HTTP** | Native `fetch` | No data-fetching or state library; `AbortController` cancels in-flight calls |
| **Wire contract** | Generated TypeScript bindings | Produced by the gateway sources in this repository (§7.1); never hand-written |
| **Drawn marks** | Authored inline SVG | `src/ui/glyph.tsx` and the inline icon in `index.html`: one stroke weight, one grid. The workspace ships no image, icon-font, or webfont files |

There is no backend framework, no database, and no ORM in this workspace — persistence is the browser's `localStorage` for the connection pair only (§3). All gateway state lives in JanusGate.

---

## 3. Connection Model

A **connection** is the pair `{ baseUrl, token }`:

- `baseUrl` — the JanusGate instance's origin (scheme + host + optional port), e.g. `https://gateway.example.com`. It is normalized once at setup: surrounding whitespace is trimmed and trailing slashes are stripped, so composing `{baseUrl}/management-api/v0/<action>` yields exactly one `/` per boundary. Only the origin is entered; the management path is fixed by the contract.
- `token` — the instance's management token, sent verbatim (§3.3). It is never rendered back in full, never written to the URL, and never logged.

### 3.1 Lifecycle

1. **Absent → setup screen.** With nothing stored, the app renders only the connection form (base URL + token). This is the first thing any visitor sees.
2. **Verify → persist.** Submitting fires the connection probe — `POST {baseUrl}/management-api/v0/providers:list` with an empty `{}` body — carrying the candidate token. Only a `200` persists the pair and enters the app; any failure (network error, non-2xx, non-JSON, a contract-violating body) keeps the user on setup with the failure reason shown.
3. **Stored → administer.** Returning visits read the pair from `localStorage` and go straight to the app. The stored pair is re-validated lazily: the first management call answering `401` (errno 2) proves the token dead, which clears the stored pair and returns to setup with an explanatory message.
4. **Forget.** An explicit action clears the stored pair and returns to setup. Editing the base URL is the same as forgetting and re-entering.

### 3.2 Addressing

- The management API root is therefore `{baseUrl}/management-api/v0`, and every route is a **`POST`** to `{management root}{path}` — the paths in the generated route table are relative to the version, e.g. `{baseUrl}/management-api/v0/providers:list`.
- Readiness to construct a path from the route table is the reason the version segment is fixed by the contract rather than user-entered: the app never concatenates a version it does not own.

### 3.3 Authentication on the wire

Every management call sends the token verbatim in the `Authorization` header **without any `Bearer` prefix**:

```text
Authorization: <token>
Content-Type: application/json
```

This matches JanusGate's management authentication exactly (a `Bearer`-prefixed value is rejected with `401`). Comparisons on the gateway side are constant-time; a mismatch yields errno 2 / HTTP 401. Requests carry no cookies — the app sends `credentials: "omit"` and never sets or reads cookies.

---

## 4. Management API Contract

### 4.1 Where the contract lives

**The generated bindings are the contract's single machine-readable home.** Every request and response payload type, the route table (path → method → request/response type), the errno taxonomy, and the ordered thinking-level vocabulary come from the gateway's own generator — the Rust sources at the repository root (§7 of the root `README.md`; the generation workflow is §7.1 below). Application code imports them and never restates a payload shape.

This section documents only what the types cannot carry: the transport-level rules, the error taxonomy's client-side meaning, and the semantics that live in the gateway's prose (two-state fields, full-state writes, single-use OAuth states).

### 4.2 Transport

- **Every route is `POST` with a JSON body**, carried by `Content-Type: application/json`. No method carries semantics; reads and writes are distinguished by the action name (`providers:list` vs `providers:create`), and a no-argument request sends an empty `{}` object.
- **The version prefix is not optional.** `{baseUrl}/management-api/v0/<path>` is the only addressable root; a path outside the version prefix is answered with an **empty 404** that carries no errno body, and a path beneath the version that matches no route is answered with the version's errno-shaped 404 (authentication applies first: an unauthorized request is rejected with 401 either way). A non-JSON body therefore means "this is not the management API", and the client reports the HTTP status alone.
- **Body limit is 2 MiB** for the whole management surface; a larger body is rejected with errno 7 (413).
- **A wrong method on a known path** is rejected with errno 8 (405) and an `Allow` header; a wrong or missing `Content-Type` is rejected with errno 9 (415) before any parsing. Both are client bugs in this app, never user-recoverable states.
- **Preflight `OPTIONS` requests are answered by the gateway's CORS layer, before authentication** — that is what makes the cross-origin flow work without a token on the preflight (§5).

### 4.3 Error taxonomy

Failures render `{"errno": <number>, "message": <string>}`. The errno is from the version's closed taxonomy, and each errno pins exactly one HTTP status:

| errno | Category | HTTP | The app's reaction |
| :--- | :--- | :--- | :--- |
| 1 | `invalid_request` | 400 | Show `message` — a rejected field or malformed body; a bug in the app's own request construction |
| 2 | `unauthorized` | 401 | Clear the stored connection and return to setup (§3.1) |
| 3 | `not_found` | 404 | The addressed row is gone: refresh the listing and show `message` |
| 4 | `conflict` | 409 | Show `message` — a taken name or a colliding model ID; resolve by editing the input |
| 5 | `upstream_error` | 502 | Show `message` — the gateway's upstream failed; the foreign status is inside the message, never the HTTP status |
| 6 | `internal` | 500 | Show `message` — a gateway fault |
| 7 | `payload_too_large` | 413 | Client bug (the app never builds bodies near 2 MiB) |
| 8 | `method_not_allowed` | 405 | Client bug |
| 9 | `unsupported_media_type` | 415 | Client bug (the app always sends `Content-Type: application/json`) |

Messages whose cause is internal render a fixed category message; the underlying cause is in the gateway's log, not on the wire. The app never invents a message of its own for a management failure — it renders the gateway's `message` verbatim.

### 4.4 Routes

All paths below are relative to `{baseUrl}/management-api/v0`. Names in the request/response columns are the generated binding types (§7).

| Path | Purpose | Request → Response |
| :--- | :--- | :--- |
| `POST /providers:list` | List providers, ordered by name; doubles as the connection probe | `ListProvidersRequest` → `ListProvidersResponse` |
| `POST /providers:create` | Create a static-key provider | `CreateProviderRequest` → `ProviderEnvelope` |
| `POST /providers:update` | Rewrite one provider's complete desired state | `UpdateProviderRequest` → `EmptyResponse` |
| `POST /providers:delete` | Delete one provider and, by cascade, its models | `DeleteProviderRequest` → `EmptyResponse` |
| `POST /oauth:start` | Begin an OAuth flow; returns the authorize URL and single-use state | `StartProviderOauthRequest` → `StartProviderOauthResponse` |
| `POST /oauth:complete` | Finish an OAuth flow by creating a provider | `CompleteProviderOauthRequest` → `ProviderEnvelope` |
| `POST /oauth:reauthorize` | Finish an OAuth flow by replacing one provider's credentials | `ReauthorizeProviderRequest` → `ReauthorizeProviderResponse` |
| `POST /models:list` | List models, optionally filtered to one provider | `ListModelsRequest` → `ListModelsResponse` |
| `POST /models:discover` | Refresh one provider's model rows from its upstream | `DiscoverModelsRequest` → `DiscoverModelsResponse` |
| `POST /models:create` | Manually add one model row with its initial overrides | `CreateModelRequest` → `ModelEnvelope` |
| `POST /models:update` | Rewrite one model's overrides | `UpdateModelRequest` → `ModelEnvelope` |
| `POST /api-keys:list` | List serving keys, ordered by row id | `ListApiKeysRequest` → `ListApiKeysResponse` |
| `POST /api-keys:create` | Issue one serving key | `CreateApiKeyRequest` → `ApiKeyEnvelope` |
| `POST /api-keys:update` | Rewrite one serving key's complete desired state | `UpdateApiKeyRequest` → `EmptyResponse` |
| `POST /api-keys:delete` | Revoke one serving key | `DeleteApiKeyRequest` → `EmptyResponse` |

### 4.5 Semantics the types cannot carry

These rules come from the gateway's design and must be honored by every call the app makes:

- **Entities are addressed by id.** Providers are addressed by `provider_id` (a request field on every route that names one); models by the pair (`provider_id`, `upstream_id`). A provider's `id` never changes, even when its `name` is changed — renaming a provider is an ordinary `providers:update` that keeps the id and every model routing. A model's routing `id` is *derived* (`id_override ?? model_prefix + upstream_id`) and recomputed at write time: it is the string serving clients resolve models by, and therefore a display value the app shows but never addresses rows with. Names and routing IDs are globally unique; a collision is a 409 that changes nothing.
- **Fields are two-state, never three.** A field is either always present (required, nullable only where it mirrors a nullable storage column: `expires_at`, `id_override`, `thinking_levels_override`) or **absent-keeps** (omissible, never `null` — the write-only secrets `api_key` and `credentials`). The bindings render these as `field: T | null` and `field?: T` respectively. Sending `null` where absence is the only alternative is a 400; sending `null` for a mirror column is meaningful and kept.
- **Updates are full-state writes.** `providers:update`, `models:update`, `models:create`, `api-keys:create`, and `api-keys:update` state the row's *complete* desired readable state; an omitted required field is a 400, not a "keep". Consequently every edit in the app is a **read-modify-write**: render the row, change the one field the administrator touched, send the whole row back. `models:create` mirrors `models:update` field for field, addressed by a fresh (`provider_id`, `upstream_id`) pair.
- **Write-only secrets are the exception**: omitting `api_key` (static-key providers) or `credentials` (OAuth providers) keeps the stored secret, so the UI never has to re-collect a secret it cannot read back. Serving keys are the counter-exception: `api-keys:list` and `api-keys:create` return the plaintext `key`, because the administrator needs it to configure serving clients — the UI shows it in full with a copy action.
- **Responses carry only what the caller cannot know.** `providers:update`, `providers:delete`, `api-keys:update`, and `api-keys:delete` answer `{}`; `oauth:reauthorize` answers the new `expires_at`; `models:discover`, `models:create`, and `models:update` answer the fresh/recomputed rows; `api-keys:create` answers the server-assigned `id` and `created_at`. The app installs what a response carries instead of refetching it, so a discovery, a model write, or a key creation needs no follow-up read; a provider or key write answering `{}` refetches the listing it can change (§6.3).
- **Nothing is created or deleted implicitly.** Model rows arrive through `models:discover` or `models:create`; `models:update` never creates a row (an unknown pair is a 404). A manually added row is indistinguishable from a discovered one afterwards: a later discovery carries its overrides over when the `upstream_id` reappears and drops the row when it does not. Providers of the two OAuth kinds are created only by `oauth:complete` — `providers:create` is static-key-only. Serving keys arrive only through `api-keys:create`; deleting a key revokes it with no cascade.
- **Renaming a provider changes its label, not its identity**; changing a model's `id_override` (or its provider's `model_prefix`) recomposes that model's public routing ID, so the previous ID stops resolving for serving clients. There is no alias layer — the stored `id` is the only routing identity.
- **OAuth states are single-use and expire after 10 minutes.** A completion consumes the state even when the token exchange then fails, so a failed completion restarts with a fresh `oauth:start`. Rejections computed before consumption (empty or taken creation name, unknown `provider_id`, provider-type mismatch on re-authorization) leave the pending flow intact.
- **A filtered `models:list` for an unknown `provider_id` yields an empty list**, not a 404 — a filter addresses no row. The app must not read "no models" as "provider missing"; the provider list is the authority on existence.
- **Thinking levels are an ordered vocabulary.** The canonical levels and their order arrive as the generated `THINKING_LEVEL_ORDER` tuple with the `ThinkingLevel` union derived from it, so the UI enumerates and orders levels from the contract instead of restating them. A model's `thinking_levels_override` filters the client-requested level; `null` means no filtering, and a non-empty array of canonical levels is the only accepted value.
- **Closed vocabularies without a published tuple are enumerated once, exhaustively.** The provider kinds and upstream formats arrive as plain unions with no runtime list, so `src/management-api/vocabulary.ts` holds one `Record<Union, …>` per vocabulary: the record type is exhaustive, so a variant added to the contract fails to compile until it is listed, and the select options, display labels, and wire-value narrowing all derive from that one declaration.

### 4.6 Client-side discipline

- **Unknown input is narrowed explicitly.** Every management response is validated before use (object shape, required keys, value types). A body that does not match the expected shape is a contract violation: the app reports it as an error rather than rendering partial data or defaulting fields. No `any`, no unchecked assertions.
- **Read-modify-write adapters construct requests field by field** — never by spreading a response row into a request body. A provider row carries `id` while its update request carries `provider_id` and restates every readable field, so a spread would smuggle `id` into the body, which the gateway's strict parsing rejects with a 400. Each adapter is one explicit mapping from a listed row to that row's update request, per union variant.
- **Errors are rendered, not swallowed.** The gateway's `message` is what the administrator sees; the errno drives control flow (§4.3).

---

## 5. CORS & Hosting Requirements

Cross-origin calls are the normal operating mode, and they impose exactly one requirement on each side:

- **Gateway side.** The administered instance's `management_api.cors.allowed_origin` must include the origin janusgate-web is served from (a specific origin such as `https://admin.example.com`, or `"*"`; an empty list disallows all cross-origin requests), otherwise the browser blocks every management call before it leaves. `management_api.cors.allow_private_network` additionally governs Private Network Access preflight for instances on private networks. Both live in the instance's JSONC configuration file; changing them means editing that file and restarting the gateway. Preflight `OPTIONS` handling and CORS response headers are entirely the gateway's job.
- **App side.** Nothing special: plain `fetch` calls carrying `Authorization` and `Content-Type: application/json` (both non-safelisted, so preflights happen — expected and answered gateway-side), with `credentials: "omit"`.
- **Static-host SPA fallback.** Client-side routes (deep links into the app) require the host to serve `index.html` for unknown paths. The app's own router treats unknown client routes as not-found.

---

## 6. Application Shell, Routes & Screens

### 6.1 The shell
The app has exactly two top-level states, chosen by the connection state (§3.1):

- **Unconnected** — the connection screen fills the viewport; nothing else exists. It states what JanusGate is, what the visitor needs before connecting, and — when the probe is refused — the app's own diagnosis of why. That screen is the first contact most operators have with the product, and a cross-origin refusal is the failure they are most likely to meet.
- **Connected** — the admin shell: a top bar (the mark, a theme switch, and a confirmed *disconnect* action), a context bar (the `Overview` / `Providers` / `Models` / `API keys` navigation, the instance address, the validity stamp of the last read, and *refresh*), the routed screen, and a quiet footer naming the connected instance.

The shell is created per connection: the connection object keys the boundary, so a different instance gets a fresh shell, a fresh management API client, and fresh screen state — nothing can leak across instances. The token is never rendered; the address and the read's timestamp are the only connection facts the shell shows.

*Refresh* bumps one counter in `src/refresh-state.ts`; the mounted screen re-runs its read when it moves, and reports the moment a read succeeded, which is what the validity stamp prints.

### 6.2 Routes
The app owns ten client-side routes; every other path renders the not-found screen. All of them are deep links, so the static host must serve `index.html` for unknown paths (§5). The OAuth flows are steps inside the provider creation and reauthorization forms, not destinations of their own.

| Path | Screen |
| :--- | :--- |
| `/` | The overview: gateway health at a glance |
| `/providers` | The provider list — rows only, no details |
| `/providers/new` | One creation page for every provider kind |
| `/providers/<provider_id>` | One provider: details, credentials, models, deletion |
| `/models`, `/models?provider=<provider_id>` | Every synced model, optionally filtered to one provider |
| `/models/new`, `/models/new?provider=<provider_id>` | One creation page for a manually added model, optionally pre-picking its provider |
| `/models/<provider_id>/<upstream_id>` | One model: facts and overrides |
| `/api-keys` | The serving-key list — rows with copy, no editing |
| `/api-keys/new` | One creation page for a serving key |
| `/api-keys/<api_key_id>` | One serving key: secret, details, deletion |

Routing is strict about identifiers: a provider filter that is not a row identifier reads as absent (the unfiltered table), a model address without a decodable upstream ID is not-found, and a path segment that is not a row identifier is not-found. A trailing slash addresses the same resource. `src/router.test.ts` keeps every `hrefs` builder and the matcher in agreement.

### 6.3 Screens
Every screen loads through one pattern: one read per mount — repeated when *refresh* is pressed — cancelled when the screen goes away, and an explicit reload after a mutation that does not answer with the affected rows. Failures render the app's diagnosis of the failure (§6.4); nothing is swallowed and no field is defaulted. A failed first read replaces the skeleton with the failure, never stacks under it.

- **Connection screen** — what JanusGate is, what the visitor must have (a running instance, its `management_api.token`, and this origin in `management_api.cors.allowed_origin` or a same-origin gateway), then the address and token fields. The token is revealable while typing. A refused probe stays on the screen with its diagnosis.
- **Overview** — gateway health at a glance: provider, model, serving, API-key, and needs-attention counts; the missing first key, the providers whose credentials need a human, and the providers with no models yet, each linking to the page that fixes it; and, on a gateway with no providers, the five steps from here to a working endpoint (the fifth issues the serving key, without which the gateway serves nothing). Request-level figures live nowhere here — the gateway keeps no per-request data (§1).
- **Providers** — one row per upstream account: name, type, model count, serving state, and credential state. Rows are links; nothing about one provider is shown or edited here. A search box narrows the list client-side.
- **New provider** — one creation page for every kind. The kind picker (OpenAI Responses API, OpenAI Chat Completions API, Gemini generateContent API, Codex OAuth, Antigravity OAuth) only swaps the credential path that follows the shared identity fields: a static-key kind is created by `providers:create` with the `type` its kind already decided, while an OAuth kind runs `oauth:start` into the authorize step and completes with the pasted code. All five land on the new provider's page.
- **Provider detail** — the provider's page, in four cards. *Details* is the edit form: it opens prefilled, tracks dirt, and saves the row's complete state back (a changed prefix recomposes unoverridden public IDs, so the models below are read again). *Credentials* states an OAuth provider's expiry and runs the reauthorization flow inline (static-key replacement lives in the details form, write-only as ever). *Models* lists the provider's models, adds one by hand, and syncs them from upstream, carrying overrides forward and refusing to wipe curated rows when upstream reports nothing. *Delete* states its cascade count and confirms in place.
- **Models** — every synced model on one page: public ID, upstream ID, serving state, and thinking levels, searchable by ID and filterable by provider (the filter is the route, `?provider=`). Rows are links; nothing is edited here. *New model* opens the creation page, pre-picking the filtered provider when one is set.
- **New model** — one creation page for a manually added row: the provider picker, the exact upstream ID, and the same three overrides the model page edits, with a live preview of the composed public ID. A warn notice states the sync risk: a later discovery keeps the overrides when the upstream ID reappears and drops the row when it does not. Creating lands on the new model's page.
- **API keys** — every serving key on one page: name, full secret with a copy action, serving state, and creation time, searchable by name or key. Rows are links; nothing is edited here.
- **New API key** — one creation page: the human label, the secret (typed or locally generated), and the serving switch. Creating lands on the new key's page.
- **API key detail** — the key's page, in three cards. *Secret* shows the full value with a copy action. *Details* is the edit form: it opens prefilled, tracks dirt, and saves the row's complete state back — rotating means Generate, then Save. *Delete* revokes the key at once (no cascade) and confirms in place, naming the last-enabled-key consequence where it applies.
- **Model detail** — one model's facts (public ID, upstream ID, provider, thinking levels) and its three overrides: the public ID override with a live preview of the composed ID, the serving switch, and the allowed thinking levels. Saving installs the row the gateway returns, because the public ID is recomposed inside that write. Clearing the override or the last level returns the row to its unfiltered state.
- **Not found** — the attempted address and the ways back.

### 6.4 Failure handling
- **401 (errno 2)** — the client hands the gateway's message to the connection service, which clears the stored pair; the connection screen explains it (§3.1). Nothing is retried with the dead token. The failure is stored whole — message, cause, and the address it addressed — so the screen can diagnose a rejection that happened while another screen was mounted.
- **Every other failure** — the message is rendered where the action was taken, in the app's own diagnosis (`src/management-api/diagnosis.ts`). The gateway's message is always kept verbatim as the detail; the app adds only what it can be certain of, and only one recovery line per case: an unauthorized answer names `management_api.token`, an upstream refusal during discovery names reauthorization, an internal failure names the gateway's stderr log, and a conflict names the identifier that collided.
- **A call the browser refused before it left** is the one case the gateway cannot explain, so the connection screen probes the instance's origin with a no-CORS request (`probeReachability`): a resolved opaque response proves the network path, which makes the failure a CORS block, and a rejected probe makes it an address nothing answers. Without that proof the diagnosis states both causes rather than guessing.
- **404 (errno 3)** — the addressed row is gone: the message is rendered and the listing that showed the row is reloaded, so the screen stops showing state the gateway no longer holds; the detail screens say so in place and offer the routes back.
- **Aborts** — navigating away cancels in-flight calls; an aborted call renders nothing.

## 7. Project Structure

```text
web/                                # the workspace root (this directory)
├── index.html                       # HTML entry: #root mount, theme bootstrap, /src/main.tsx
├── DESIGN.md                        # The built visual world: tokens, materials, and rules
├── .impeccable/                     # Development-only: the surface brief and the review captures
├── src/
│   ├── main.tsx                     # Entry point: inits the theme, mounts <App />, imports tailwind.css
│   ├── app.tsx                      # Connection gate, admin shell, navigation, route dispatch
│   ├── router.ts                    # Route matching, link builders, history navigation
│   ├── refresh-state.ts             # The refresh stamp and the refresh request every screen shares
│   ├── theme.ts                     # Light/dark/system preference, resolution, persistence
│   ├── format.ts                    # Timestamps, credential validity, allowed-level phrasing
│   ├── config.ts                    # Single home for non-secret tunables (e.g. storage keys)
│   ├── tailwind.css                 # Tailwind v4 entry: the console's light/dark tokens and base styles
│   ├── vite-env.d.ts                # Vite client types reference
│   ├── connection/                  # The connection pair: model, storage, reducer, service
│   ├── management-api/              # The wire contract and its use: client, validation, adapters, outcome, errors, diagnosis, generated bindings
│   ├── ui/                          # The console's parts: controls, feedback, layout, glyphs, tables, shared form pieces
│   └── screens/                     # One module per route
```


Test modules (`*.test.ts`) live next to the modules they cover and are not listed above.

### 7.1 Generated bindings

The wire contract enters this workspace as **generated TypeScript**, produced by the gateway's own generator from the Rust sources at the repository root — the gateway declares each route once (method, path, request type, response type) and emits the payload declarations plus the route table. Nothing about the management API is hand-written here.

```bash
npm run generate-bindings
```

The script is one command, inline in `package.json`, and does exactly this:

1. remove the previous output directory, so no stale artifact from an older layout survives;
2. run the gateway's bindings generator against the repository's local working tree (`cargo run --manifest-path ../Cargo.toml`, with the `management-api-typescript-bindings` feature), writing `src/management-api/generated/v0.ts`.

Properties of this arrangement, all deliberate:

- **The gateway is the only source of truth.** The Rust types are the schema; the generator derives the bindings from them, so the app cannot hold a stale copy of a payload shape without failing to compile against the regenerated file.
- **The output is a build artifact, not source.** `src/management-api/generated/` is gitignored (and therefore also outside Biome's scope, which reads `.gitignore`). A fresh checkout must run `npm run generate-bindings` before `npm run typecheck`, `npm run build`, or `npm test` once application code imports the bindings.
- **The bindings regenerate from the local working tree.** A management-API contract change and its web adaptation are one atomic change: the generator runs against the same tree being edited — no clone, no network, no push-then-regenerate round trip. Regenerating against a different gateway revision means checking out that revision and running the script again.
- **It is a dev-time step, not a build step.** Running it requires a Rust toolchain; the first invocation compiles the generator, and subsequent ones are incremental. It is never wired into `npm run build`, so the deployed artifact is built from TypeScript alone.
- **The generator is idempotent and deterministic**: the same gateway revision always renders byte-identical output.

### 7.2 Implementation status

The app is implemented end to end: the connection lifecycle (§3), all fifteen management routes (§4.4), every screen of §6, and the domain-logic suites of §11. No placeholder screen remains.

---

## 8. Configuration

There are two kinds of configuration, strictly apart:

- **Connection (runtime, per browser).** The `{ baseUrl, token }` pair from §3, stored in `localStorage` under the key in `src/config.ts`. No build-time or deployment-time values exist — the same `target/web/dist/` artifact administers any instance. Rotating a gateway's management token invalidates stored connections; the app recovers through the 401 path (§3.1).
- **Theme preference (runtime, per browser).** Light, dark, or the operating system's — stored in `localStorage` under its own key in `src/config.ts`, painted before the first frame by the inline bootstrap in `index.html`, and holding no secret.
- **Tunables (build-time, in code).** Every non-secret configurable value lives in `src/config.ts`, its single home. Changing one means editing the module and rebuilding. No runtime environment variables are read (`import.meta.env` is not a configuration mechanism here) — a static artifact that behaved differently per host via env injection would break the one-artifact-anywhere promise.

No secrets ever appear in code, in `src/config.ts`, or in this workspace. The management token exists only in the administrator's browser storage and on the wire to the configured instance.

---

## 9. Conventions

The normative rule list is `AGENTS.md`; this section orients:

- **Imports:** `/` is the `web/` root. Prefer root imports (`/src/...`) unless the module is in the same folder (`./module`) or a sibling subtree (`../folder/module`). Generated files are exempt from this rule but should be imported through the same root style.
- **Types:** strict TypeScript throughout — no `any`, and no assertion a preceding runtime check does not justify; unknown input (notably every management API response) is narrowed through explicit validation; exported functions and component helpers carry explicit return types.
- **Generics:** all-or-nothing naming per parameter list (single-letter when simple; otherwise every parameter multi-letter with a `T`-prefix for types / `F`-prefix for callbacks describing its role).
- **Booleans** carry predicate-style prefixes (`is`, `can`, `has`, `should`); names are unabbreviated (`id` and the usual short auxiliaries excepted).
- **Quality gates:** after every code change run `npm run check` (Biome over the whole repository, plus this workspace's `tsc --noEmit`) and `npm run test:web` from the repository root. TDD: write tests first for non-trivial logic; keep tests cohesive and aimed at first-party domain logic (URL normalization, response validation, read-modify-write adapters, connection state transitions, failure diagnosis, formatting), never at framework primitives.
- **Visual language:** every colour comes from the token set in `src/tailwind.css` (light by default, dark on demand); no ad-hoc values, no shadows, no state carried by colour alone (every status is icon plus words). `web/DESIGN.md` is the world's rulebook.
- **Docs first:** `README.md` is the source of truth — every behavior change updates it alongside the code.

---

## 10. Security Notes

- **Public app, secret-gated control.** Anyone opening the deployed URL sees the connection screen (or the admin shell); controlling an instance additionally requires its management token. The app's hosting needs no access control of its own for this to hold — but the token lives in that browser's `localStorage`, so only open the app in browsers you trust (any script running on the app's origin could read it; the app therefore ships zero third-party assets beyond its bundled code).
- **Token handling.** The token is sent only to the configured `baseUrl` (plus its `/management-api/v0/*` paths) and nowhere else; it never appears in URLs, logs, or error reports. A 401 anywhere clears it from storage immediately rather than retrying with it.
- **No sessions, no cookies** — none are set, sent, or read anywhere, matching the gateway; cross-origin calls explicitly opt out of credentials.
- **Transport.** Serve janusgate-web over HTTPS and point it at HTTPS gateway instances; otherwise the token crosses the network in cleartext. The app does not warn or enforce this — the deployment does.
- **CORS as allowlist.** A gateway listing `"*"` in `management_api.cors.allowed_origin` accepts management calls from any website. Because the token lives in this app's origin-scoped storage (not in a cookie), a foreign origin cannot borrow it — but prefer pinning the exact app origin anyway.
- **Upstream secrets stay in the gateway.** The app collects OAuth authorization codes (which it relays immediately to complete a flow) but never stores, displays, or logs them; provider credentials and upstream API keys are write-only from the app's perspective and are never returned by any route. Serving keys are the exception: the gateway returns them in full so they can be copied into client configuration — they are shown as data with a copy action, and never placed in a URL, a log, or storage beyond the gateway's own answers in memory.

---

## 11. Development

### Prerequisites

- Node.js and npm for the app itself.
- A Rust toolchain (cargo) for `npm run generate-bindings` only — required once per checkout and again after every management-API contract change in the gateway.

### Scripts

All scripts below run from `web/`. The repository root is an npm workspace wrapping this package: `npm run dev:web`, `build:web`, and `test:web` at the root delegate here, and the root `npm run check` / `npm run fix` run Biome (root `biome.jsonc`, whole repository) followed by this package's typecheck.

| Script | Purpose |
| :--- | :--- |
| `npm run generate-bindings` | Regenerate `src/management-api/generated/v0.ts` from the local gateway sources (§7.1) |
| `npm run dev` | Local dev server with hot reload |
| `npm run build` | Production static build into `target/web/dist/` |
| `npm run typecheck` | `tsc --noEmit` |
| `npm test` | Vitest run (domain-logic suites, §11) |

### First run and workflow

```bash
npm install                     # at the repository root — installs every workspace
npm run generate-bindings -w web   # once per checkout; needs cargo, regenerates from the local gateway
npm run dev:web                 # iterate against a real JanusGate instance
npm run check                   # Biome over the repository + web typecheck, after every change
npm run build:web               # produce target/web/dist/ for static hosting
```

Local development talks to a real JanusGate instance: enter its base URL and token in the setup screen. Ensure that instance's `management_api.cors.allowed_origin` includes the dev server's origin (or `"*"`), or the browser will block the probe call. Deep links in dev are served by Vite's history fallback; the static host needs the equivalent (§5).

### What the tests cover
Vitest protects the parts a browser-level check cannot pin down precisely: base URL normalization and storage validation, the connection reducer and its service transitions (including a submission that must not race another), response validation for every route's payload (including the api-keys listing and envelope), the read-modify-write adapters (including the api-key update and the model creation mirroring the update shape), routing (including every `hrefs` builder against the matcher), the client's request construction and error classification against a stubbed `fetch` (including the api-keys and model creation paths), the failure diagnosis (each errno, the creation-conflict fixes, the contract mismatch, and the three reachability readings), and the formatting of credential validity and allowed-level phrasing.

Rendering and behavior are verified against a real JanusGate instance in a browser: connecting and resuming, the probe's failure paths, the overview with live counts, the provider list and its search, creating each provider kind, opening and editing a provider, discovery, a prefix change that recomposes model IDs, setting and clearing serving, reauthorization, issuing/rotating/revoking serving keys (including generate-and-copy), manually adding a model from both entries, 401 recovery, rows deleted behind a screen's back, browser history, deep links, the not-found screen, the models table's search and filter, the model editor, and both light and dark themes at desktop and phone widths.

## 12. Invariants

1. The same static artifact administers any JanusGate instance; no instance address or token is ever baked into a build.
2. The connection pair exists only in the browser (`localStorage` + memory) and on the wire to the configured instance — never in code, config, URLs, or logs.
3. Every management request is a `POST` to `{baseUrl}/management-api/v0/<path>` carrying `Authorization: <token>` verbatim (no `Bearer` prefix) and `Content-Type: application/json`; any 401 clears the stored pair and returns to setup.
4. Payload shapes are never restated in this workspace: the generated bindings are the contract, and application code compiles against them. A body that violates the expected shape is reported as an error, never partially rendered.
5. Reads that mutate (updates, deletions, creations) follow the gateway's full-state and two-state rules: complete desired state on update and create, omission — never `null` — for write-only secrets, explicit field-by-field construction rather than spreading a row into a request. Serving-key secrets are the readable exception: they are restated like every other field and shown in full with a copy action.
6. No request, response, token, or usage data is persisted anywhere except the connection pair in `localStorage`.
7. The wire contract is owned by the gateway's `README.md` §7 (the repository root) and its generated bindings; on any conflict with this document, they win.
8. The artifact carries no asset files — no fonts, images, or third-party scripts — so the token in this origin's storage is reachable by no code the app did not ship; the app's marks are drawn SVG and its type is the platform's own stack.
9. Every failure is explained where it happened: the gateway's message is rendered verbatim, the app's own diagnosis adds only what it can prove, and no failure is swallowed or replaced with a generic notice.
