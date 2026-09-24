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

- **Goals:** one-connection administration of providers, models, serving keys, usage reads, and OAuth flows; zero build-time or deployment-time configuration — the same artifact points at any instance; minimal moving parts (static files + browser `fetch` + one dev-time codegen step).
- **Excluded:** serving inference traffic (clients call the gateway's `/api/*` directly, never through this app); user accounts, roles, or sessions in the app; multi-tenancy; any credential handling beyond the management token and the serving keys themselves — upstream OAuth secrets and provider API keys are minted inside JanusGate by its own flows, and the app only relays the administrator's input, while serving-key secrets are typed or locally generated in the app and relayed to the gateway. Request content — prompts, completions, headers — is never shown anywhere: usage reads carry ids and numeric facts only.

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

**The generated bindings are the contract's single machine-readable home.** Every request and response payload type, the route table (path → method → request/response type), the errno taxonomy, and the closed vocabularies (provider kinds, discovery states, request-accounting sets) come from the gateway's own generator — the Rust sources at the repository root (§7 of the root `README.md`; the generation workflow is §7.1 below). Application code imports them and never restates a payload shape.

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
| `POST /models:list` | List every model row (vanished included), optionally filtered to one provider | `ListModelsRequest` → `ListModelsResponse` |
| `POST /models:discover` | Refresh one provider's model rows from its upstream | `DiscoverModelsRequest` → `DiscoverModelsResponse` |
| `POST /models:create` | Manually add one model row, or claim a vanished one | `CreateModelRequest` → `ModelEnvelope` |
| `POST /models:update` | Rewrite one model row's two writable columns | `UpdateModelRequest` → `ModelEnvelope` |
| `POST /models:delete` | Delete one model row | `DeleteModelRequest` → `EmptyResponse` |
| `POST /usage:list` | List recorded request-accounting rows, newest first, under the shared filter | `ListUsageRequest` → `ListUsageResponse` |
| `POST /usage:query` | Aggregate recorded rows into buckets | `QueryUsageRequest` → `QueryUsageResponse` |
| `POST /api-keys:list` | List serving keys, ordered by row id | `ListApiKeysRequest` → `ListApiKeysResponse` |
| `POST /api-keys:create` | Issue one serving key | `CreateApiKeyRequest` → `ApiKeyEnvelope` |
| `POST /api-keys:update` | Rewrite one serving key's complete desired state | `UpdateApiKeyRequest` → `EmptyResponse` |
| `POST /api-keys:delete` | Revoke one serving key | `DeleteApiKeyRequest` → `EmptyResponse` |

### 4.5 Semantics the types cannot carry

These rules come from the gateway's design and must be honored by every call the app makes:

- **Entities are addressed by id.** Providers by `provider_id`, API keys by `api_key_id`, models by their never-recycled row identifier `model_id` — a request field on every route that names one. A provider's `id` never changes, even when its `name` is changed — renaming a provider is an ordinary `providers:update` that keeps the id and every model routing. A model's routing `public_id` is *derived* (`id_override ?? model_prefix + upstream_id`) and recomposed at write time: it is the string serving clients resolve models by, and therefore a display value the app shows but never addresses rows with. Names and routing IDs are globally unique; a collision is a 409 that changes nothing. A deleted row's number is never handed to a later row, so usage already recorded keeps attributing by id.
- **Fields are two-state, never three.** A field is either always present (required, nullable only where it mirrors a nullable storage column: `expires_at`, `id_override`, an `api_log` row's `http_status` and five token columns) or **absent-keeps** (omissible, never `null` — the write-only secrets `api_key` and `credentials`, the `usage` reads' paging fields, and every field of the shared usage filter (README §7.1)). The bindings render these as `field: T | null` and `field?: T` respectively. Sending `null` where absence is the only alternative is a 400; sending `null` for a mirror column is meaningful and kept.
- **Machine-consumed text is routing-safe.** Every field the gateway routes, addresses, or presents to an upstream — upstream IDs, public ID overrides, model prefixes, serving-key secrets, upstream static keys, hand-replaced OAuth credential fields, and static-key base URLs — holds only printable ASCII without whitespace (no spaces, no control characters, no non-ASCII text such as CJK); a base URL is additionally a non-empty `http://` or `https://` address. The app validates every such field where it is typed — the offending value is named inline and the submit stays disabled — so a rejection the gateway would answer with 400 never leaves the browser. Human labels (provider and serving-key names) stay free-form.
- **Updates are full-state writes.** `providers:update`, `api-keys:create`, and `api-keys:update` state the row's *complete* desired readable state; `models:update` states the complete state of the row's two writable columns, and `models:create` the complete desired initial state — an omitted required field is a 400, not a "keep". Consequently every edit in the app is a **read-modify-write**: render the row, change the one field the administrator touched, send the whole row back. `models:create` is addressed by a fresh (`provider_id`, `upstream_id`) natural key.
- **Write-only secrets are the exception**: omitting `api_key` (static-key providers) or `credentials` (OAuth providers) keeps the stored secret, so the UI never has to re-collect a secret it cannot read back. Serving keys are the counter-exception: `api-keys:list` and `api-keys:create` return the plaintext `key`, because the administrator needs it to configure serving clients — the UI shows it in full with a copy action.
- **Responses carry only what the caller cannot derive.** `providers:update`, `providers:delete`, `api-keys:update`, `api-keys:delete`, and `models:delete` answer `{}`; `oauth:reauthorize` answers the new `expires_at`; `models:discover`, `models:create`, and `models:update` answer the fresh rows with their recomposed `public_id`; `api-keys:create` answers the server-assigned `id` and `created_at`. A `usage:list` page's next cursor is the last returned row's `id`, which the client derives. The app installs what a response carries instead of refetching it, so a discovery, a model write, or a key creation needs no follow-up read; a write answering `{}` refetches the listing it can change (§6.3). A row never carries a verdict the caller derives — the three-input effective serving state is computed client-side from rows the app already fetches — and a usage row or bucket never carries a name: labels resolve through the entity lists.
- **Nothing is created or deleted implicitly.** Model rows arrive through `models:discover` or `models:create`; `models:update` never creates a row (an unknown `model_id` is a 404) and never touches `discovery_state`, so it cannot restore a vanished row — recovery is an upstream reappearance or a `models:create` claim of the vanished natural key, which resumes service under the requested state. Discovery never deletes a row: an unlisted row is marked `vanished`, keeping its curated columns, and only `models:delete` removes one (a deleted `discovered` row returns as a new row with a fresh identity on the next discovery). Providers of the two OAuth kinds are created only by `oauth:complete` — `providers:create` is static-key-only. Serving keys arrive only through `api-keys:create`; deleting a key revokes it with no cascade. An upstream reporting no models rejects the discovery before any write, so curated rows are never mass-vanished by an empty report.
- **Renaming a provider changes its label, not its identity**; changing a model's `id_override` (or its provider's `model_prefix`) recomposes that model's public routing ID, so the previous ID stops resolving for serving clients. There is no alias layer — the stored `id` is the only routing identity.
- **OAuth states are single-use and expire after 10 minutes.** A completion consumes the state even when the token exchange then fails, so a failed completion restarts with a fresh `oauth:start`. Rejections computed before consumption (empty or taken creation name, unknown `provider_id`, provider-type mismatch on re-authorization) leave the pending flow intact.
- **A filtered `models:list` for an unknown `provider_id` yields an empty list**, not a 404 — a filter addresses no row. The app must not read "no models" as "provider missing"; the provider list is the authority on existence.
- **Closed vocabularies are enumerated once, exhaustively.** The provider kinds, the discovery states, and the request-accounting sets (`ApiLogEndpointType`, `ApiLogOutcome`, `UsageGroupBy`) arrive as plain unions with no runtime list, so `src/management-api/vocabulary.ts` holds one `Record<Union, …>` per vocabulary: the record type is exhaustive, so a variant added to the contract fails to compile until it is listed, and the select options, display labels, and wire-value narrowing all derive from that one declaration. Reasoning effort is the gateway's own closed vocabulary — `none | minimal | low | medium | high | xhigh | max`, parsed per surface and rendered verbatim into the target upstream's parameter, never mapped across vocabularies — and the UI states no levels anywhere: no model row and no accounting row carries one.

### 4.6 Client-side discipline

- **Unknown input is narrowed explicitly.** Every management response is validated before use (object shape, required keys, value types). A body that does not match the expected shape is a contract violation: the app reports it as an error rather than rendering partial data or defaulting fields. No `any`, no unchecked assertions.
- **Read-modify-write adapters construct requests field by field** — never by spreading a response row into a request body. A provider row carries `id` while its update request carries `provider_id` and restates every readable field, so a spread would smuggle `id` into the body, which the gateway's strict parsing rejects with a 400. Each adapter is one explicit mapping from a listed row to that row's update request, per wire field set — one branch for the three static-key kinds, one per OAuth kind.
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
- **Connected** — the admin shell: a top bar (the mark, a theme switch, and a confirmed *disconnect* action), a context bar (the `Overview` / `Providers` / `Models` / `API keys` / `Usage` navigation, the instance address, the validity stamp of the last read, and *refresh*), the routed screen, and a quiet footer naming the connected instance.

The shell is created per connection: the connection object keys the boundary, so a different instance gets a fresh shell, a fresh management API client, and fresh screen state — nothing can leak across instances. The token is never rendered; the address and the read's timestamp are the only connection facts the shell shows.

*Refresh* bumps one counter in `src/refresh-state.ts`; the mounted screen re-runs its read when it moves, and reports the moment a read succeeded, which is what the validity stamp prints.

### 6.2 Routes
The app owns fifteen client-side routes; every other path renders the not-found screen. All of them are deep links, so the static host must serve `index.html` for unknown paths (§5). The OAuth flows are steps inside the provider creation and reauthorization forms, not destinations of their own.

| Path | Screen |
| :--- | :--- |
| `/` | The overview: gateway health at a glance |
| `/providers` | The provider list — rows only, no details |
| `/providers/new` | One creation page for every provider kind |
| `/providers/<provider_id>` | One provider: details, models, usage |
| `/providers/<provider_id>/edit` | Edit one provider: settings, credentials, deletion |
| `/models`, `/models?provider=<provider_id>` | Every model, optionally filtered to one provider |
| `/models/new`, `/models/new?provider=<provider_id>` | One creation page for a manually added model (or a claim of one the upstream no longer reports); the provider is fixed when opened from a provider's page, and a vanished model's claim link adds an optional `&upstream=<upstream_id>` (URL-encoded) that prefills the claim |
| `/models/<model_id>` | One model: facts and usage |
| `/models/<model_id>/edit` | Edit one model: settings and deletion |
| `/usage` | Every dispatched request, newest first, under the filter the route carries |
| `/usage/query` | Request accounting as aggregates |
| `/api-keys` | The serving-key list — rows with copy, no editing |
| `/api-keys/new` | One creation page for a serving key |
| `/api-keys/<api_key_id>` | One serving key: secret, facts, usage |
| `/api-keys/<api_key_id>/edit` | Edit one serving key: settings and deletion |

Routing is strict about identifiers: a provider filter that is not a row identifier reads as absent (the unfiltered table), an `upstream` prefill that is not a legal model identifier reads as absent, and a path segment that is not a row identifier is not-found — including the previous two-segment model addresses, which no longer resolve. A row identifier is a plain decimal integer within the safe-integer range; a digit string beyond it names no row the gateway could hold and reads as absent wherever it appears — a path segment, a models filter, and a usage filter alike. A trailing slash addresses the same resource. `src/router.test.ts` keeps every `hrefs` builder and the matcher in agreement.

### 6.3 Screens
Every screen loads through one pattern: one read per mount — repeated when *refresh* is pressed or the screen's route identity moves — cancelled when the screen goes away, and an explicit reload after a mutation that does not answer with the affected rows. The three edit screens' settings forms track dirt, and a refresh respects it: an open dirty form is never reset, and a row that changed on the gateway while the form is dirty is held back with a notice stating the operator can *Save* (writes the edits over the gateway's version) or *Revert* (picks the gateway's version up); a save re-baselines the form onto the values it sent and keeps the row held until the reload it triggers lands — a failed reload leaves the hold standing rather than resting the screen on the rows the save overwrote, and the first read to land after the save releases it. Failures render the app's diagnosis of the failure (§6.4); nothing is swallowed and no field is defaulted. The skeleton is the read harness's first-read window — open exactly while a read is in flight, nothing has been read yet, and nothing stands in the notice — so a failed first read closes it and the standing failure keeps it closed while the re-read runs, never stacked under.

- **Connection screen** — what JanusGate is, what the visitor must have (a running instance, its `management_api.token`, and this origin in `management_api.cors.allowed_origin` or a same-origin gateway), then the address and token fields. The token is revealable while typing. A refused probe stays on the screen with its diagnosis.
- **Overview** — the gateway's health and its recent traffic. Five figures, each a door to the page that explains it: providers, models routing (the three-input effective status, counted from the rows the app already fetches), API keys, and the requests and errors recorded in the last 24 hours. Below them, a bar chart of the last fourteen UTC days — the gateway's own day boundary, zero-filled so a quiet day is a bar at the axis rather than a gap — with a readout line that states the window's total and its peak day until a bar is read, then that day and its figure; every bar is focusable and carries its day and figure as its accessible name, so no figure is readable only by hovering. Then the "Needs attention" list, whose count is exactly the number of items it can show — the missing first key, the providers whose credentials need a human, and the providers with no models yet — with the "Everything looks healthy" notice in its place at zero; each listed item links to the page that fixes it. On a gateway with no providers, the figures and the chart give way to the five steps from here to a working endpoint (the fifth issues the serving key, without which the gateway serves nothing). The page ends with the **Endpoints** card: the instance's base URL plus one path per outward surface (`/api/openai-v1`, `/api/gemini-v1beta`, `/api/anthropic-v1`), each copyable — the one fact the management API never states, since a request routes by public model ID alone and never by surface.
- **Providers** — one row per upstream account: name, type, model count, enabled state (Enabled or Disabled), and the credential badge (Static key, Valid, Expired, or Stale). Rows are links; nothing about one provider is shown or edited here. A search box narrows the list client-side.
- **New provider** — one creation page for every kind. The kind picker (OpenAI Responses API, OpenAI Chat Completions API, Gemini generateContent API, Codex OAuth, Antigravity OAuth) only swaps the credential path that follows the shared identity fields: a static-key kind is created by `providers:create` with the `type` its kind already decided, while an OAuth kind runs `oauth:start` into the authorize step and completes with the pasted code. The name is suggested from the kind and stays free-form; the model prefix defaults to that name plus one trailing dash, and a custom prefix is sent exactly as typed. Every provider starts enabled; switching one off lives on its edit page. All five land on the new provider's page.
- **Provider detail** — the provider's page, in three cards. *Details* states what it is, read-only. *Models* lists the provider's models, adds one by hand, and discovers them from upstream, its progress, success, and failure stated on this card where the action was taken — custom names carry forward, counted in the success notice only when some are kept, and an empty model list starts one discovery on its own. *Usage* aggregates this provider's requests per day. Nothing is changed here — *Edit provider* opens the edit page.
- **Provider edit** — the provider's changes, in up to three cards. *Settings* opens prefilled with the Enabled switch first, tracks dirt, and saves the complete settings back: the name cannot be emptied (the same non-empty rule the serving-key forms name inline), a custom prefix is saved exactly as typed, and a changed prefix recomposes the public IDs of the models without an override (the save states that recomposition only when it actually recomposes one). *Credentials* states an OAuth provider's expiry and runs the reauthorization flow inline (static-key replacement lives in the settings form, write-only as ever). *Delete* states what deletion removes — the cascade count, or, over a provider with no models, the fact that nothing else is removed — and confirms in place.
- **Models** — every model on one page: public ID, upstream ID, serving state, and discovery state, searchable by ID and filterable by provider (the filter is the route, `?provider=`). Rows are links; nothing is edited here. *Create model* opens the creation page, fixing the filtered provider when one is set.
- **New model** — one creation page for a manually added model: its identity (the provider picker, or the fixed provider name when opened from a provider's page; the exact upstream ID) and its *Model settings* (the Enabled switch first, then the public ID override with a live preview of the composed public ID). A warn notice states the claim semantic: naming a provider and upstream ID the upstream no longer reports claims that model back under the settings here. A claim link carried from a vanished model's page prefills the upstream ID — never forced — and the field's hint states that submitting claims that ID back. Creating lands on the new model's page.
- **Usage** — every dispatched request, newest first, under the shared filter: a *Filter* card carrying a rolling window (all time, last 24 hours, 7 days, or 30 days — resolved when the stream is read, so a shared link keeps meaning the same span) and the same provider, model, key, endpoint, outcome, stream-mode, and HTTP-status fields the aggregates page offers, plus one outcome choice — *Failures (upstream or internal)* — that names exactly what the overview's error figure counts. The filter is the route: applying it navigates, so a filtered stream is linkable, shareable, and walkable with the browser's back button, while the unfiltered stream stays at exactly `/usage`; every page of a stream carries the filter its first page carried, so a rolling window is resolved once per cycle rather than per page. A stream that matches nothing states that the filter located no rows and offers to clear it — a different state from a gateway that has recorded nothing. Labels resolve through the current entity lists, and a row deleted after recording is named by its kind — Deleted model, Deleted provider, Deleted key — never by a guessed name or number. A *Load more* continuation extends the stream: a failed page keeps the continuation available (the failure notice is the signal; only a page shorter than the full page size ends the stream), the end of the stream states what it is the end of — every row this filter matches, or every recorded row — a refresh while more is loading takes the stream over wholesale rather than appending stale rows, and a refresh that fails leaves the continuation on the filter its rows in hand were listed under, never pairing their cursor with a filter they did not answer.
- **Detail usage** — every detail page (provider, model, API key) carries a *Usage* card: that subject's totals plus one row per day, newest first, counted through `usage:query` with the subject's id filter.
- **Usage aggregates** — one bucket per distinct combination of the requested dimensions inside the filter: any set of the eight dimensions (day, model, provider, API key, endpoint, outcome, stream mode, HTTP status) toggled as switches — none selected totals every row — over the shared filter, an inclusive from and an exclusive to in this browser's zone, provider/model/key narrows, an endpoint, an outcome, a stream mode, and comma-separated HTTP statuses. Each bucket's composite key renders one part per requested dimension, in request order, resolved through the entity lists and the vocabularies; the table renders under the dimensions of the query that produced it, so toggling a switch never reinterprets the standing answer. A submit runs the same cycle a read runs — the entity lists load first, so a query superseding an in-flight read never labels its buckets against unloaded lists. Token totals skip rows where the upstream reported no usage.
- **API keys** — every serving key on one page: name, full secret with a copy action, enabled state (On or Off), and creation time, searchable by name or key. Rows are links; nothing is edited here.
- **New API key** — one creation page: the Enabled switch first, the human label, and the secret (typed or locally generated). The label is required where it is typed — the gateway's only name rule is non-empty — so an emptied name is named inline and creation stays disabled. Creating lands on the new key's page.
- **API key detail** — the key's page, in three cards. *Secret* shows the full value with a copy action; a clipboard the browser refuses is stated, pointing at the selectable value for a hand copy. *Details* states its name, creation time, and enabled state, read-only. *Usage* aggregates this key's requests per day. Nothing is changed here — *Edit API key* opens the edit page.
- **API key edit** — the key's changes, in two cards. *Settings* opens prefilled with the Enabled switch first, tracks dirt, and saves the complete settings back — the name cannot be emptied (the same non-empty rule, named inline), and rotating means Generate, then Save. *Delete* revokes the key at once (no cascade) and confirms in place, naming the last-enabled-key consequence where it applies.
- **Model detail** — one model's facts (public ID, upstream ID, provider, discovery state, enabled state) and its usage per day. A model the upstream no longer reports states its two recovery paths in place as links: *Discover again on the provider's page* opens that page, where discovery runs, and the claim opens the Create model page with this model's provider already fixed and its upstream ID carried along to prefill the claim. Nothing is changed here — *Edit model* opens the edit page.
- **Model edit** — one model's settings and its deletion. The form opens prefilled with the Enabled switch first, tracks dirt, and saves the complete settings back — saving installs the model the gateway returns, because the public ID is recomposed inside that write. Deletion is administrator-only and confirmed where it states its consequences; the model's number is never recycled, so recorded usage keeps attributing to it.
- **Not found** — the attempted address and the ways back.

### 6.4 Failure handling
- **401 (errno 2)** — the client hands the gateway's message to the connection service, which clears the stored pair; the connection screen explains it (§3.1). Nothing is retried with the dead token. The failure is stored whole — message, cause, and the address it addressed — so the screen can diagnose a rejection that happened while another screen was mounted.
- **Every other failure** — the message is rendered where the action was taken, in the app's own diagnosis (`src/management-api/diagnosis.ts`). The gateway's message is kept verbatim as the detail whenever the gateway rendered one; a call the browser refused before it left has none, and the diagnosis's own wording is the detail instead (next bullet). The app adds only what it can be certain of, and only one recovery line per case: an unauthorized answer names `management_api.token`, an upstream refusal during discovery names the recovery the provider's kind dictates — reconnecting for an OAuth provider, a base-URL and API-key check for a static-key provider — an internal failure names the gateway's stderr log, and a conflict names the identifier that collided.
- **A call the browser refused before it left** is the one case the gateway cannot explain, so the connection screen probes the instance's origin with a no-CORS request (`probeReachability`): a resolved opaque response proves the network path, which makes the failure a CORS block, and a rejected probe makes it an address nothing answers. Without that proof the diagnosis states both causes rather than guessing.
- **404 (errno 3)** — the addressed row is gone: the message is rendered and the listing that showed the row is reloaded, so the screen stops showing state the gateway no longer holds; the detail screens say so in place and offer the routes back.
- **Aborts** — navigating away cancels in-flight calls; an aborted call renders nothing.

## 7. Project Structure

```text
web/                                # the workspace root (this directory)
├── index.html                       # HTML entry: #root mount, theme bootstrap, /src/main.tsx
├── DESIGN.md                        # The built visual world: tokens, materials, and rules
├── .impeccable/                     # Development-only: the surface brief, the design-system brief, and the question logs
├── src/
│   ├── main.tsx                     # Entry point: inits the theme, mounts <App />, imports tailwind.css
│   ├── app.tsx                      # Connection gate, admin shell, navigation, route dispatch
│   ├── router.ts                    # Route matching, link builders, history navigation
│   ├── usage-filter.ts              # The shared accounting filter: form vocabulary, route encoding, wire filter
│   ├── refresh-state.ts             # The refresh stamp and the refresh request every screen shares
│   ├── theme.ts                     # Light/dark/system preference, resolution, persistence
│   ├── format.ts                    # Timestamps, credential validity, token counts, date-time bounds
│   ├── config.ts                    # Single home for non-secret tunables (e.g. storage keys)
│   ├── tailwind.css                 # Tailwind v4 entry: the console's light/dark tokens and base styles
│   ├── vite-env.d.ts                # Vite client types reference
│   ├── connection/                  # The connection pair: app-service, model, service, state
│   ├── management-api/              # The wire contract and its use: client, validation, adapters, outcome, errors, diagnosis, identifiers, oauth-code, outward-surfaces, provider-defaults, serving, vocabulary, generated bindings
│   ├── ui/                          # The console's parts: app-link, attributes, bar-chart, clipboard, controls, feedback, layout, glyphs, tables, shared form pieces
│   └── screens/                     # One module per route, plus the screens' shared fragments
```


Test modules (`*.test.ts(x)`) live next to the modules they cover and are not listed above. Inside `src/screens/`, eight shared fragments serve every route's module: `read.ts` (the screen read harness §6.3), `read-notice.tsx` (the standing read notice every screen states, §6.3), `failure.ts` (the failure presentation §6.4 renders), `usage-read.ts` (the usage domain's read calls — the entity listings and the `usage:query` call whose bucket keys are proven inside the cycle), `usage-labels.ts` (usage labels resolved through the entity lists), `usage-stats.tsx` (the detail pages' usage card), `usage-pagination.ts` (the usage stream's cursor and continuation), and `overview-stats.ts` (the overview's daily window, its zero-filled series, and its rolling figures). At the `src/` root beside the router that consumes it, `usage-filter.ts` — the shared accounting filter (its form vocabulary, its route encoding, and the wire filter it builds) — serves the router's usage route and the usage screens alike.

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

The app is implemented end to end: the connection lifecycle (§3), all eighteen management routes (§4.4), every screen of §6, and the domain-logic suites of §11. No placeholder screen remains.

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

Arguments after `--` pass through to the underlying tool: `npm run dev -- --host` from `web/`, or `npm run dev:web -- --host` from the repository root (the root `dev:web`, `build:web`, and `test:web` delegates forward arguments the same way).

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
Vitest protects the parts a browser-level check cannot pin down precisely: base URL normalization and storage validation, the machine-consumed field composition rules, the connection reducer and its service transitions (including a submission that must not race another), response validation for every route's payload (including the usage listing and aggregation), the read-modify-write adapters (including the `model_id`-addressed model update and the vanished-claim creation shape), the provider creation request that carries a typed custom prefix byte-for-byte, the client-derived effective serving definition, routing (including every `hrefs` builder against the matcher, the row-identifier model addresses, the usage routes, and the model-new route's claim-back `upstream` prefill), the client's request construction and error classification against a stubbed `fetch` (including the model deletion and usage paths), the failure diagnosis (each errno, the creation-conflict fixes, the contract mismatch, and the three reachability readings), the OAuth authorization code's extraction from a pasted bare code or callback URL (parameter or fragment, percent-encoded, and the pastes that carry no readable code), the provider defaults (each kind's base name, the name suggestion that counts up over taken names and reuses the lowest freed suffix, and the name-derived default prefix), the call-outcome wrapper (a carried result, a rendered failure keeping the gateway's message and errno, a foreign failure without an errno, an abort as no outcome at all), usage label resolution for deleted rows, the usage pagination state (the cursor taken from the stream's last row and the per-page continuation that ends only on a short page), the serving-key name validation (only the empty name is rejected), the provider detail screen's automatic first discovery (started once for an empty model list, never for a populated one, and never read again when the discovery settles after the screen is gone), the screen read harness (calls answered in call order, the first failure's diagnosis carried out of a multi-call read and adopted whole from a nested read, so a composite call's leaf diagnosis survives nesting, an abort ending the cycle ahead of any failure, the row-identity predicates that keep an unchanged re-read from flipping object identity, and the held row a dirty form keeps against a changed read), the first-read skeleton window held open exactly while a read is in flight with nothing read yet and nothing standing in the notice (a standing failure keeps it closed through the re-read, on kept-notice screens too), the three edit screens' dirty-hold behavior (a dirty form kept across a refresh, a row that changed on the gateway held behind the Save-or-Revert notice, a save re-baselining the form onto the values it sent, a revert picking the gateway's latest version up), the usage query screen's refresh racing its first read (the loading state standing until the newer cycle settles) with a submit that reads the entity lists before the query lands, the usage stream's continuation guards (a superseded page discarded, a load refused while a read cycle is in flight, a failed page keeping both the continuation and the cursor, and a failed refresh leaving the continuation on the rows' own filter rather than the failed cycle's), the models table's serving-badge ladder (one rung per cause: the row's own switch, then the vanished row's suspension, then the provider's hold), the clipboard helper's fallback ladder (the async Clipboard API first, the legacy `execCommand` textarea second, an honest boolean either way) with the shared copy button's state machine (a copied label reverts shortly; a refusal stands until a later copy replaces it), the delete confirmation card's state machine (armed to its question, then deleting; disarming when the confirm answers that the row survived, holding its post-confirm state when the row is gone and the screen owns the navigation), the shared usage filter's route encoding (every field round-tripped through the address, a value outside its vocabulary read as absent, a row identifier beyond the safe-integer range read as absent, the failure pair kept as one choice while any other set reads as absent, and a rolling window bounded at the moment it is read), the overview's figures (the fourteen-day window's UTC boundaries, the zero-filled axis that keeps a quiet day a bar rather than a gap, the busiest day, and the rolling counts that call exactly the outcomes the listing's *Failures* choice filters by) with the overview screen's own reading (five figures that are doors, the chart and its summary, and the three endpoint paths), the usage listing's filtered stream (the route's filter carried into every page under one resolution of its rolling window, no filter sent at all when nothing is chosen, a filtered empty answer stated as a filter that matched nothing, and clearing the filter navigating back to `/usage`), and the formatting of credential validity, token counts, and date-time bounds.

Rendering and behavior are verified against a real JanusGate instance in a browser: connecting and resuming, the probe's failure paths, the overview with live counts, its five figures, its fourteen-day chart, and the endpoint paths it copies, the provider list and its search, creating each provider kind, opening a provider and editing it on its edit page, discovery (including models the upstream no longer reports, its progress and failure notices, and the automatic first discovery), a prefix change that recomposes model IDs, the Enabled switches, claiming back a model the upstream no longer reports, deleting a model, reauthorization, issuing/rotating/revoking serving keys (including generate-and-copy), manually adding a model from both entries, the detail usage cards, the usage stream with its continuation, its filter card, and the filtered address it navigates to, and the aggregation query with its filters, 401 recovery, rows deleted behind a screen's back, browser history, deep links, the not-found screen, the models table's search and filter, the settings forms, and both light and dark themes at desktop and phone widths.

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
10. The outward surface paths the console prints are the gateway's own (root `README.md` §1 *Outward Protocols*) and the only endpoint advice the app gives: routing is by public model ID alone, so no screen may present a provider kind or a model as bound to one surface.
