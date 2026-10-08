# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

- **Operators of self-hosted JanusGate instances.** The product's users are people who run their own gateway: the project's author and other self-hosters who installed JanusGate from the repository. Their job is to turn a fresh headless gateway into a working endpoint for the client they already use — creating upstream accounts, discovering physical models, configuring optional aliases, managing account pools, and recovering credentials that expire. Each has the instance's single management token and administers alone; there is no second seat.
- **Strangers arriving at the official deployment.** The project ships an officially deployed instance of the frontend that anyone may use, and many operators will meet JanusGate there having read only the READMEs. The app is the first — often only — part of the product they see, and it must be usable without knowledge of the management API's shape.
- Both groups are the same audience in different moods: the same operator who reads the repository is the one who later opens the UI to fix an expired account. Device and session shape were not constrained.

## Product Purpose

JanusGate is a self-hosted, lightweight AI gateway written in Rust. It exposes three outward serving API families — OpenAI-compatible (`/api/openai-v1`), Gemini (`/api/gemini-v1beta`), and Anthropic (`/api/anthropic-v1`) — backed by heterogeneous upstream accounts, and a versioned management JSON API (`/management-api/v0/*`). The gateway is deliberately headless: no frontend, no static assets, no accounts, no sessions.

janusgate-web is the missing control plane — the official, statically hosted single-page application through which one operator administers a JanusGate instance entirely over the network. It is the sanctioned UI for the management API, lives in this repository as the `web/` workspace, and is deployed separately from the gateway it administers.

Success for the web UI: an operator can connect to a running gateway and reach a correctly routed, working endpoint — accounts created, models discovered, models automatically exposed and optional aliases configured, pool membership and quota availability understood, expired credentials replaced, and the endpoint path plus the issued serving key in hand — without hand-editing the configuration file, reading gateway logs, or calling the API themselves.

## Positioning

Gateways that ship an admin UI ship it inside the server: the UI is bound to one deployment, its version, and its origin. JanusGate is headless and its frontend is a separate artifact — **one static build with no build-time or deployment-time configuration administers any instance, anywhere**. The operator proves control by presenting the instance's own management token, verified by a probe against that instance; nothing about the instance is baked into the artifact, and the app has no backend of its own.

Two mechanisms a neighboring product could not truthfully copy: the artifact is instance-agnostic by construction, and the frontend **compiles against TypeScript bindings generated from the gateway's own Rust sources**, so its wire contract cannot drift into a hand-maintained parallel.

The gateway it fronts carries the same posture: strict upstream model passthrough with explicit public route names and ordered account pools (no automatic model mapping or cross-vendor fallback), upstream accounts — ChatGPT and Google OAuth subscriptions as well as static API keys — as first-class identities, and no request content retained (numeric request accounting aside). Its three outward families are independent of those accounts: **every surface serves every upstream format**, so one public model ID is the single routing key no matter which client dialect asks for it — and the console never presents a provider kind as bound to an endpoint.

## Operating Context

- **The gateway is headless and file-configured.** It runs from a JSONC configuration file (`-c`/`--config`) with hot reload, optional TLS, and logging to stderr. Every fact the UI administers lives in the gateway's in-process SQLite-compatible database (Turso) and is reachable only through the management API.
- **Cross-origin is the normal case.** The app's origin and the gateway's origin differ by design, so the instance's `management_api.cors.allowed_origin` must include the app's origin (or `*`), and `management_api.token` is the credential. Fixing CORS means editing that file and restarting the gateway — the UI cannot do it.
- **Connection is per browser.** The pair `{baseUrl, token}` is entered on first visit, verified by one probe call (`providers:list`), then kept only in the browser's `localStorage`. Returning visits skip setup; a 401 clears the pair and returns to setup. One instance at a time; another instance means forgetting the current connection.
- **Real deployment constraints.** The build output is plain static files, so the host needs an SPA fallback for deep links; HTTPS on both sides is expected because the management token crosses the network; there is no server-side component to configure.
- **Development.** `npm run dev:web` (Vite) drives the app against a real gateway instance; `npm run generate-bindings` (requires cargo) regenerates the wire types from the local gateway sources. Rendering is verified in a browser against a real instance.
- **Gateway non-goals that bound the UI's scope:** no multi-tenancy, no billing or rate limiting, no content logging, no cross-vendor fallback, no automatic model mapping, no outbound proxying.

## Capabilities and Constraints

**Administered today**, entirely through `/management-api/v0/*` — every route `POST` with a JSON body, `Authorization: <token>` verbatim (no `Bearer` prefix):

- **Outward surfaces** — three client dialects over one effective-model set: OpenAI-compatible (`/api/openai-v1`: chat completions, responses), Gemini (`/api/gemini-v1beta`: generateContent, interactions), and Anthropic (`/api/anthropic-v1`: messages), each with its own auth and its own native model listing. Nothing in the management API names a surface: a request routes by public model ID alone, and a public ID containing `/` or `:` is unreachable through the URL-pathed Gemini generateContent route (the gateway rejects both characters in newly written routing tokens).
- **Providers** — credential-bearing account instances bound to immutable provider definitions; list with enabled state and OAuth expiry; create a static-key account with a definition reference, name, base URL, API key, and serving switch; rewrite complete readable state, with credentials write-only; delete, cascading to physical models, direct routes, pool memberships, and quota restrictions. Definitions describe connection recipes and one of the five wire formats independently of product presets.
- **OAuth accounts** — start the selected definition's authorization-code or device flow to create an account or reauthorize an existing account in place. The frontend supplies researched product presets; the gateway executes authored recipes without recognizing product names.
- **Models** — list (optionally filtered to one provider); discover a provider's models from upstream as a mark-plus-upsert that rewrites the row's `discovery_state` (`discovered` / `vanished` / `manual`) and its four discovery-owned capability base columns — never deleting a row, never touching the administrator's columns, and refusing an empty upstream report; edit its serving switch and four capability overrides addressed by its never-recycled row id; delete a row; create a row by its natural key `(provider_id, upstream_id)`, which also claims a vanished row as the manual recovery path.
- **Public routes and pools** — Inventory discovery automatically exposes standalone provider models as `provider.model_prefix + upstream_id`. Pool membership suppresses those provider-prefixed entries; each pool exposes the union of member upstream IDs using its own `model_prefix`, routing only to members with the exact model. Add alias opens optional direct-route creation with an editable suggested name; bulk publication creates the same routes individually and reports conflicts. A route has its own public ID and switch, targeting either one physical model or a pool plus an exact upstream ID. Target edits preserve route identity and may retain its public ID. Pools have a name, model prefix, switch, and ordered account membership committed as one complete update; missing models remain visible and are never synthesized.
- **Availability** — account restrictions are shared across pools and shown separately from administrator switches and physical inventory state. Loading a view reads stored evidence without polling upstream quota. Explicit Refresh reconciles quota; Reset clears restrictions and gives one automatic-refresh interval of grace. Past reset times require rechecking and never prove replenishment. Established history stays on its account or receives an explicit availability error; only an unbound first-dispatch classified quota rejection permits selecting the next account.
- **Serving-API keys** — list, issue, rewrite a key's complete state (rename, rotate, serving switch), and revoke each key individually; the plaintext secret is a stored field the gateway answers with in the clear on both the listing and the creation, and the console surfaces it in full with a copy action for the administrator's serving-client configuration.
- **Usage reads** — list recorded request-accounting rows newest first with cursor pagination, and aggregate them into buckets over any set of nine dimensions (day, public route, physical model, provider, API key, endpoint, outcome, stream mode, HTTP status) — one composite bucket key per requested dimension, in request order, with an empty set totalling the whole selection. Both reads share one filter type: a time window plus multi-value filters over the same dimensions, up to 100 values each, an empty list rejected. Rows and buckets carry ids and numeric facts only — labels resolve through the entity lists. Rows distinguish the first upstream HTTP status from the actual outward HTTP status; abandoned quota attempts have no outward status.
- **Derived facts the UI states under the gateway's documented definitions:** physical inventory readiness (`provider enabled && model enabled && not vanished`), derived client-side from fetched rows; direct-route publication counts separate from inventory readiness; pool member eligibility for an exact upstream ID, with independent stored quota restrictions and refresh outcomes; explicit public IDs authored on routes; each capability's effective declaration (`override ?? base` — the value the model listings render, unknown omitted) with its provenance (overridden, discovered, unknown); and OAuth credential freshness.
- **Vocabulary fixed by the domain:** provider, model prefix, upstream ID, public ID, discovery and its three states, serving key, request outcome, endpoint, reauthorization, reasoning level, capability, declaration. Reasoning effort is a **closed vocabulary** — `none | minimal | low | medium | high | xhigh | max` — parsed by the gateway, written verbatim into the target upstream's own parameter, never mapped across vocabularies; it is also the value domain of the per-model capability declarations, whose effective sets the model listings render — a declaration, never an enforcement. No accounting row carries one.

**Planned, confirmed by the user, mechanism not yet decided:**

- **Observability of gateway state and credential health** — OAuth expiry and refresh state, upstream reachability, discovery results, model counts, serving state. Request-level insight is no longer part of this pending work: it ships through the administered usage reads above, fed by request accounting that retains one numeric row per dispatched request (README §3 *ApiLog*, governed by the presence of the `api_logs` configuration section), while content — prompts, completions, headers, upstream error bodies — remains forbidden by the stateless-execution principle.

**Resolved (2026-09-22):** model rows are addressed by their never-recycled row id; the previous `(provider_id, upstream_id)` deep links are not preserved. Discovery state (`discovered` / `vanished` / `manual`) plus the client-derived effective status is the status language; a vanished row states its recovery paths (upstream reappearance or a `models:create` claim).

**Resolved (2026-09-24, after the multi-surface refactor):**

- **The console carries the endpoint reference.** The instance's base URL plus `/api/openai-v1`, `/api/gemini-v1beta`, and `/api/anthropic-v1`, each copyable, on the overview — where an operator meets the gateway's shape before a key exists. The READMEs stay normative; the console stops being silent about where a client points.
- **The Usage listing gains the shared filter.** The raw unfiltered stream is no longer the design: both usage reads expose the same filter set.
- **The overview reports recent traffic beside gateway state.** Its figures are provider, enabled-public-route, ready-physical-model, API-key, recent-request (24 h), and recent-error (24 h) counts — each figure a link to the page that owns it — over a bar chart of requests per day for the last 14 UTC days.

**Explicitly out of scope, confirmed:**

- Editing the gateway's configuration file (listen, TLS, CORS, management token) from the UI.
- Managing several instances from one app at a time.
- Any account, role, or permission model in the frontend — one management token is the whole access model.
- Carrying inference traffic through the UI, and persisting any request or response content.

**Hard constraints, not preferences:**

- **Zero third-party assets.** No CDN scripts, remote fonts, images, or analytics: the management token lives in this origin's `localStorage`, so any script running on the origin could read it. Everything the app uses is bundled or inlined.
- **The contract is generated, never restated.** Application code compiles against the bindings; a response violating its expected shape is reported as an error, never partially rendered.
- **Write semantics are the gateway's, verbatim:** full-state updates (omitting a required field is a rejection, not a keep), two-state fields (absent-keeps, never `null`), and write-only secrets that are never read back (serving-key secrets are the readable exception, restated like any other field).

## Brand Commitments

- The names **JanusGate** and **janusgate-web**, the repository and package names (`janusgate`, `janusgate-web`), and the GitHub organization **EAimTY** are fixed facts.
- **The Janus motif is part of the identity.** Janus is the two-faced Roman god of gates and doorways, and the name should visibly keep carrying that idea through the product's life; the idea is a commitment, its implementation is open.
- The current mark — an inline SVG favicon of two pillars on a dark field, drawn inline only because the app ships no asset files — **is not an implementation of that commit**. No logotype, typeface, palette, or brand voice exists or is committed. The repository contains no brand asset files.
- **Resolved (2026-09-18, shipped):** the previous web UI visual world (the dark route-setting panel) was replaced rather than refined, per the earlier decision. The current `DESIGN.md` — the quiet light-first console — is the visual authority; the old look survives nowhere and constrains nothing.

## Evidence on Hand

- **Normative documentation:** root `README.md` (gateway design; §7 defines the management API including its seven route-design rules and errno taxonomy; §8 covers configuration, hot reload, and logging) and `web/README.md` (frontend design, connection model, routes, screens, invariants).
- **Machine-checkable contract:** `web/src/management-api/generated/v0.ts`, generated from the gateway sources by `npm run generate-bindings`; it is the source of truth for every payload shape the UI sends or receives.
- **A live official deployment exists and is open to anyone** (user-confirmed); its URL is not recorded anywhere in the repository.
- **Absent, and not to be fabricated:** no license file, no screenshots, demo video, or product tour, no testimonials, customer names, adoption numbers, benchmarks, or press. The READMEs are the only evidence of the product's behavior, and the running app is the only demonstration.

## Product Principles

1. **One token is the whole access model.** No accounts, roles, sessions, or permissions will be invented in the frontend. Anything requiring an identity concept belongs somewhere else.
2. **The gateway owns product truth; the app owns its presentation.** Wire shapes come from generated bindings, derived state comes from the gateway, and the app never restates a contract it does not own — a violation surfaces as an error rather than being smoothed over.
3. **Secrets flow in, never back out.** The management token, upstream provider API keys, and OAuth codes are relayed and never displayed, stored, logged, or placed in a URL; those credentials are write-only from the app's perspective. Serving-key secrets are the gateway's deliberate readable exception: returned so the administrator can copy them into serving clients, shown in full with a copy action, and never placed in a URL or persisted by the app.
4. **Nothing about an instance is baked in.** The same artifact must administer any gateway, so no address, token, or instance-specific value may reach a build.
5. **It must explain itself to a stranger.** The official deployment is the product's front door, so the gateway's vocabulary and state have to be legible in place, to an operator who has read nothing.

## Accessibility & Inclusion

The web UI is held to **WCAG 2.2 AA**: keyboard-complete operation, correct semantics and focus order, and contrast verified against the standard. This commitment applies especially to the official public deployment, where operators arrive without training and cannot be assumed to use a pointer.

Serving keys restrict stable public entries with a required allow/deny default, typed provider/pool defaults, and model-route overrides. Permission inherits route → owner → key; pool permission covers every eligible member and future members. Global availability remains separate. Key forms start deny, load inventory, and save model access with ordinary settings; explicit intent survives renames, retargets, filtering, and dirty refresh holds, while deleted references require removal. Key detail explains effective access and its source. Deletion cascades permission rules and preserves historical accounting.
