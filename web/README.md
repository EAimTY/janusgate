# janusgate-web — Standalone Web Frontend for JanusGate

> Single source of truth for this project's design and architecture. Contracts and invariants stated here are **normative** for this repository.
>
> The JanusGate gateway is normative for everything wire-level. Its `README.md` §7 defines the management API, and the **generated TypeScript bindings (§6) are that contract's machine-readable form** — payload shapes are deliberately not restated in this document, so the two can never drift. When something here disagrees with the gateway, the gateway wins.

---

## 1. Overview & Scope

**janusgate-web** is a standalone, statically-hosted single-page application (SPA) that administers a [JanusGate](https://github.com/EAimTY/janusgate) instance through its versioned management JSON API (`/management-api/v0/*`).

JanusGate itself is **headless**: it exposes an OpenAI-compatible serving API (`/api/openai-v1`) and a management API (`/management-api/v0/*`), and ships no frontend of its own. janusgate-web is the missing control plane UI — deployed **separately, anywhere that serves static files**, talking to any JanusGate instance over the network.

### Deployment model

```text
┌─────────────────────────┐         cross-origin fetch          ┌──────────────────────────┐
│ janusgate-web (static)  │  ─────────────────────────────────▶ │ JanusGate (headless)     │
│ any static host, public │   POST /management-api/v0/<action>  │ anywhere reachable       │
│ no backend of its own   │   Authorization: <token>            │ management_api.token     │
└─────────────────────────┘   Content-Type: application/json    └──────────────────────────┘
```

- **Deploy anywhere.** The build output (`dist/`) is plain static files (`index.html` + JS + CSS). Any static host works; no server-side rendering, no edge functions, no backend of its own.
- **Public by default.** Anyone who can reach the deployed URL can open the app. Opening it grants nothing by itself: every management call requires the target instance's management token, which the app never ships with.
- **First-visit connection setup.** The app holds no connection on first entry, so it presents a connection screen asking for exactly two values: the JanusGate **management API base URL** (e.g. `https://gateway.example.com`) and its **management token** (matching `management_api.token` in that instance's configuration). The pair is verified with one probe call (`POST /management-api/v0/providers:list`) before it is accepted and persisted.
- **Connection persists locally.** The accepted pair lives only in the browser's `localStorage` (key in `src/config.ts`). Returning visits skip setup; a "forget" action clears the stored pair and returns to setup. The app administers one instance at a time.
- **Cross-origin is the normal case.** The app's origin and the gateway's origin are expected to differ, and CORS on the gateway makes that work (§5). No proxying, no same-origin requirement, no backend-for-frontend.

### Goals & non-goals

- **Goals:** one-connection administration of providers, models, and OAuth flows; zero build-time or deployment-time configuration — the same artifact points at any instance; minimal moving parts (static files + browser `fetch` + one dev-time codegen step).
- **Excluded:** serving inference traffic (clients call the gateway's `/api/*` directly, never through this app); user accounts, roles, or sessions in the app; usage tracking, metrics, or request logging; multi-tenancy; any credential handling beyond the management token itself — upstream OAuth secrets and API keys are minted inside JanusGate by its own flows, and the app only relays the administrator's input.

---

## 2. Technology Stack

| Layer | Choice | Notes |
| :--- | :--- | :--- |
| **Language** | TypeScript (strict) | `tsc --noEmit` is part of `npm run check`; strict flags per `tsconfig.json` |
| **UI framework** | SolidJS v2 (`solid-js` + `@solidjs/web`) | Fine-grained reactivity, no virtual DOM; JSX with `jsxImportSource: @solidjs/web` |
| **Styling** | Tailwind CSS v4 (via `@tailwindcss/vite`) | Utility-first; single entry `src/tailwind.css` (`@import "tailwindcss"`) |
| **Build / dev server** | Vite | Static output to `dist/`; the host needs an SPA fallback (§5) |
| **Lint & format** | Biome | `biome.jsonc` (mirrors the JanusGate web stack); `npm run check` / `npm run fix` |
| **Tests** | Vitest | `npm test`; TDD per `AGENTS.md` |
| **HTTP** | Native `fetch` | No data-fetching or state library; `AbortController` cancels in-flight calls |
| **Wire contract** | Generated TypeScript bindings | Produced by the gateway repository itself (§6); never hand-written |

There is no backend framework, no database, and no ORM in this repository — persistence is the browser's `localStorage` for the connection pair only (§3). All gateway state lives in JanusGate.

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

**The generated bindings are the contract's single machine-readable home.** Every request and response payload type, the route table (path → method → request/response type), the errno taxonomy, and the ordered thinking-level vocabulary come from [`EAimTY/janusgate`](https://github.com/EAimTY/janusgate)'s own generator (§6). Application code imports them and never restates a payload shape.

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

All paths below are relative to `{baseUrl}/management-api/v0`. Names in the request/response columns are the generated binding types (§6).

| Path | Purpose | Request → Response |
| :--- | :--- | :--- |
| `POST /providers:list` | List providers, ordered by name; doubles as the connection probe | `ListProvidersRequest` → `ListProvidersResponse` |
| `POST /providers:create` | Create an `openai_compatible` provider | `CreateProviderRequest` → `ProviderEnvelope` |
| `POST /providers:update` | Rewrite one provider's complete desired state | `UpdateProviderRequest` → `EmptyResponse` |
| `POST /providers:delete` | Delete one provider and, by cascade, its models | `DeleteProviderRequest` → `EmptyResponse` |
| `POST /oauth:start` | Begin an OAuth flow; returns the authorize URL and single-use state | `StartProviderOauthRequest` → `StartProviderOauthResponse` |
| `POST /oauth:complete` | Finish an OAuth flow by creating a provider | `CompleteProviderOauthRequest` → `ProviderEnvelope` |
| `POST /oauth:reauthorize` | Finish an OAuth flow by replacing one provider's credentials | `ReauthorizeProviderRequest` → `ReauthorizeProviderResponse` |
| `POST /models:list` | List models, optionally filtered to one provider | `ListModelsRequest` → `ListModelsResponse` |
| `POST /models:discover` | Refresh one provider's model rows from its upstream | `DiscoverModelsRequest` → `DiscoverModelsResponse` |
| `POST /models:update` | Rewrite one model's overrides | `UpdateModelRequest` → `ModelEnvelope` |

### 4.5 Semantics the types cannot carry

These rules come from the gateway's design and must be honored by every call the app makes:

- **Entities are addressed by id.** Providers are addressed by `provider_id` (a request field on every route that names one); models by the pair (`provider_id`, `upstream_id`). A provider's `id` never changes, even when its `name` is changed — renaming a provider is an ordinary `providers:update` that keeps the id and every model routing. A model's routing `id` is *derived* (`id_override ?? model_prefix + upstream_id`) and recomputed at write time: it is the string serving clients resolve models by, and therefore a display value the app shows but never addresses rows with. Names and routing IDs are globally unique; a collision is a 409 that changes nothing.
- **Fields are two-state, never three.** A field is either always present (required, nullable only where it mirrors a nullable storage column: `expires_at`, `id_override`, `thinking_levels_override`) or **absent-keeps** (omissible, never `null` — the write-only secrets `api_key` and `credentials`). The bindings render these as `field: T | null` and `field?: T` respectively. Sending `null` where absence is the only alternative is a 400; sending `null` for a mirror column is meaningful and kept.
- **Updates are full-state writes.** `providers:update` and `models:update` state the row's *complete* desired readable state; an omitted required field is a 400, not a "keep". Consequently every edit in the app is a **read-modify-write**: render the row, change the one field the administrator touched, send the whole row back.
- **Write-only secrets are the exception**: omitting `api_key` (compatible providers) or `credentials` (OAuth providers) keeps the stored secret, so the UI never has to re-collect a secret it cannot read back.
- **Responses carry only what the caller cannot know.** `providers:update` and `providers:delete` answer `{}`; `oauth:reauthorize` answers the new `expires_at`; `models:discover` and `models:update` answer the fresh/recomputed rows. The app therefore never refetches what it just wrote — a discovery or a model update already returned the authoritative rows.
- **Nothing is created or deleted implicitly.** Model rows exist only through `models:discover`; `models:update` never creates a row (an unknown pair is a 404). Providers of the two OAuth kinds are created only by `oauth:complete` — `providers:create` is `openai_compatible`-only.
- **Renaming a provider changes its label, not its identity**; changing a model's `id_override` (or its provider's `model_prefix`) recomposes that model's public routing ID, so the previous ID stops resolving for serving clients. There is no alias layer — the stored `id` is the only routing identity.
- **OAuth states are single-use and expire after 10 minutes.** A completion consumes the state even when the token exchange then fails, so a failed completion restarts with a fresh `oauth:start`. Rejections computed before consumption (empty or taken creation name, unknown `provider_id`, provider-type mismatch on re-authorization) leave the pending flow intact.
- **A filtered `models:list` for an unknown `provider_id` yields an empty list**, not a 404 — a filter addresses no row. The app must not read "no models" as "provider missing"; the provider list is the authority on existence.
- **Thinking levels are an ordered vocabulary.** The canonical levels and their order arrive as the generated `THINKING_LEVEL_ORDER` tuple with the `ThinkingLevel` union derived from it, so the UI enumerates and orders levels from the contract instead of restating them. A model's `thinking_levels_override` filters the client-requested level; `null` means no filtering, and a non-empty array of canonical levels is the only accepted value.

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

## 6. Project Structure

```text
janusgate-web/
├── index.html                       # HTML entry: #root mount + /src/main.tsx module script
├── src/
│   ├── main.tsx                     # Entry point: mounts <App /> on #root, imports tailwind.css
│   ├── app.tsx                      # Application shell (placeholder until screens land)
│   ├── config.ts                    # Single home for non-secret tunables (e.g. storage key)
│   ├── tailwind.css                 # Tailwind v4 entry (@import "tailwindcss")
│   ├── vite-env.d.ts                # Vite client types reference
│   └── management-api/
│       └── generated/               # GENERATED, gitignored — `npm run generate-bindings` writes v0.ts here
├── vite.config.ts                   # Vite + SolidJS + Tailwind plugins; output dist/
├── tsconfig.json                    # Strict TypeScript; `/` maps to the repository root
├── biome.jsonc                      # Lint + format (mirrors the JanusGate web stack)
├── package.json                     # Scripts and dependencies (no backend, no deploy tooling)
├── AGENTS.md                        # Agent working rules (this repo's conventions)
├── CLAUDE.md                        # Points at AGENTS.md
└── README.md                        # This file
```

### 6.1 Generated bindings

The wire contract enters this repository as **generated TypeScript**, produced by the gateway repository's own generator — the gateway declares each route once (method, path, request type, response type) and emits the payload declarations plus the route table. Nothing about the management API is hand-written here.

```bash
npm run generate-bindings
```

The script is one command, inline in `package.json`, and does exactly this:

1. create a temporary directory (`mktemp -d`);
2. shallow-clone `https://github.com/EAimTY/janusgate` at the `rust` branch (override with `JANUSGATE_REF=<branch|tag|rev>`);
3. remove the previous output directory, so no stale artifact from an older layout survives;
4. run the gateway's bindings generator with the `management-api-typescript-bindings` feature, writing `src/management-api/generated/v0.ts`;
5. remove the temporary directory — on success and on failure alike — and propagate the generator's exit status.

Properties of this arrangement, all deliberate:

- **The gateway is the only source of truth.** The Rust types are the schema; the generator derives the bindings from them, so the app cannot hold a stale copy of a payload shape without failing to compile against the regenerated file.
- **The output is a build artifact, not source.** `src/management-api/generated/` is gitignored (and therefore also outside Biome's scope, which reads `.gitignore`). A fresh checkout must run `npm run generate-bindings` before `npm run typecheck`, `npm run build`, or `npm test` once application code imports the bindings.
- **The clone is the pushed state of the branch.** Bindings mirror the remote branch, never a local working tree: an uncommitted change in a local janusgate checkout is invisible here until it is pushed. Bumping the contract is therefore push-then-regenerate, and `JANUSGATE_REF` can pin a tag or revision when reproducibility matters.
- **It is a dev-time step, not a build step.** Running it requires `git`, a Rust toolchain, and network access, and a cold run compiles the whole gateway (minutes). It is never wired into `npm run build`, so the deployed artifact is built from TypeScript alone.
- **The generator is idempotent and deterministic**: the same gateway revision always renders byte-identical output.

### 6.2 Current scaffold status

Deliberately, only the toolchain shell exists: the app renders a placeholder proving SolidJS + Tailwind + Vite build and serve correctly. **No business screens are implemented** — connection setup, provider management, model management, OAuth paste-back dialogs, and the management API client are future work to be built against §3–§4 of this document.

---

## 7. Configuration

There are two kinds of configuration, strictly apart:

- **Connection (runtime, per browser).** The `{ baseUrl, token }` pair from §3, stored in `localStorage` under the key in `src/config.ts`. No build-time or deployment-time values exist — the same `dist/` artifact administers any instance. Rotating a gateway's management token invalidates stored connections; the app recovers through the 401 path (§3.1).
- **Tunables (build-time, in code).** Every non-secret configurable value lives in `src/config.ts`, its single home. Changing one means editing the module and rebuilding. No runtime environment variables are read (`import.meta.env` is not a configuration mechanism here) — a static artifact that behaved differently per host via env injection would break the one-artifact-anywhere promise.

No secrets ever appear in code, in `src/config.ts`, or in the repository. The management token exists only in the administrator's browser storage and on the wire to the configured instance.

---

## 8. Conventions

The normative rule list is `AGENTS.md`; this section orients:

- **Imports:** `/` is the repository root. Prefer root imports (`/src/...`) unless the module is in the same folder (`./module`) or a sibling subtree (`../folder/module`). Generated files are exempt from this rule but should be imported through the same root style.
- **Types:** strict TypeScript throughout — no `any`, no unchecked assertions; unknown input (notably every management API response) is narrowed through explicit validation; exported functions and component helpers carry explicit return types.
- **Generics:** all-or-nothing naming per parameter list (single-letter when simple; otherwise every parameter multi-letter with a `T`-prefix for types / `F`-prefix for callbacks describing its role).
- **Booleans** carry predicate-style prefixes (`is`, `can`, `has`, `should`); names are unabbreviated (`id` and the usual short auxiliaries excepted).
- **Quality gates:** after every code change run `npm run check` (Biome + `tsc --noEmit`) and `npm test`. TDD: write tests first for non-trivial logic; keep tests cohesive and aimed at first-party domain logic (URL normalization, response validation, read-modify-write adapters, connection state transitions), never at framework primitives.
- **Docs first:** `README.md` is the source of truth — every behavior change updates it alongside the code.

---

## 9. Security Notes

- **Public app, secret-gated control.** Anyone opening the deployed URL sees the setup screen (or the admin shell); controlling an instance additionally requires its management token. The app's hosting needs no access control of its own for this to hold — but the token lives in that browser's `localStorage`, so only open the app in browsers you trust (any script running on the app's origin could read it; the app therefore ships zero third-party assets beyond its bundled code).
- **Token handling.** The token is sent only to the configured `baseUrl` (plus its `/management-api/v0/*` paths) and nowhere else; it never appears in URLs, logs, or error reports. A 401 anywhere clears it from storage immediately rather than retrying with it.
- **No sessions, no cookies** — none are set, sent, or read anywhere, matching the gateway; cross-origin calls explicitly opt out of credentials.
- **Transport.** Serve janusgate-web over HTTPS and point it at HTTPS gateway instances; otherwise the token crosses the network in cleartext. The app does not warn or enforce this — the deployment does.
- **CORS as allowlist.** A gateway listing `"*"` in `management_api.cors.allowed_origin` accepts management calls from any website. Because the token lives in this app's origin-scoped storage (not in a cookie), a foreign origin cannot borrow it — but prefer pinning the exact app origin anyway.
- **Upstream secrets stay in the gateway.** The app collects OAuth authorization codes (which it relays immediately to complete a flow) but never stores, displays, or logs them; provider credentials and API keys are write-only from the app's perspective and are never returned by any route.

---

## 10. Development

### Prerequisites

- Node.js and npm for the app itself.
- `git` and a Rust toolchain (cargo) for `npm run generate-bindings` only — required once per checkout and again after every gateway contract bump.

### Scripts

| Script | Purpose |
| :--- | :--- |
| `npm run generate-bindings` | Clone janusgate and regenerate `src/management-api/generated/v0.ts` (§6.1) |
| `npm run dev` | Local dev server with hot reload |
| `npm run build` | Production static build into `dist/` |
| `npm run preview` | Serve the production build locally |
| `npm run typecheck` | `tsc --noEmit` |
| `npm run check` | Biome check + typecheck (run after every change) |
| `npm run fix` | Biome auto-fix (unsafe) + typecheck |
| `npm test` | Vitest run (passes with no tests until suites land) |

### First run and workflow

```bash
npm install
npm run generate-bindings   # once per checkout; needs git + cargo
npm run dev                 # iterate against a real JanusGate instance
npm run check               # lint + types, after every change
npm run build               # produce dist/ for static hosting
```

Local development talks to a real JanusGate instance: enter its base URL and token in the setup screen. Ensure that instance's `management_api.cors.allowed_origin` includes the dev server's origin (or `"*"`), or the browser will block the probe call. Deep links in dev are served by Vite's history fallback; the static host needs the equivalent (§5).

---

## 11. Invariants

1. The same static artifact administers any JanusGate instance; no instance address or token is ever baked into a build.
2. The connection pair exists only in the browser (`localStorage` + memory) and on the wire to the configured instance — never in code, config, URLs, or logs.
3. Every management request is a `POST` to `{baseUrl}/management-api/v0/<path>` carrying `Authorization: <token>` verbatim (no `Bearer` prefix) and `Content-Type: application/json`; any 401 clears the stored pair and returns to setup.
4. Payload shapes are never restated in this repository: the generated bindings are the contract, and application code compiles against them. A body that violates the expected shape is reported as an error, never partially rendered.
5. Reads that mutate (updates, deletions) follow the gateway's full-state and two-state rules: complete desired state on update, omission — never `null` — for write-only secrets, explicit field-by-field construction rather than spreading a row into a request.
6. No request, response, token, or usage data is persisted anywhere except the connection pair in `localStorage`.
7. The wire contract is owned by JanusGate's `README.md` §7 and its generated bindings; on any conflict with this document, they win.
