# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

- **Operators of self-hosted JanusGate instances.** The product's users are people who run their own gateway: the project's author and other self-hosters who installed JanusGate from the repository. Their job is to turn a fresh headless gateway into a working OpenAI-compatible endpoint — creating upstream accounts, discovering and curating routes, and recovering credentials that expire. Each has the instance's single management token and administers alone; there is no second seat.
- **Strangers arriving at the official deployment.** The project ships an officially deployed instance of the frontend that anyone may use, and many operators will meet JanusGate there having read only the READMEs. The app is the first — often only — part of the product they see, and it must be usable without knowledge of the management API's shape.
- Both groups are the same audience in different moods: the same operator who reads the repository is the one who later opens the UI to fix an expired account. Device and session shape were not constrained.

## Product Purpose

JanusGate is a self-hosted, lightweight AI gateway written in Rust. It exposes an OpenAI-compatible serving API (`/api/openai-v1`) backed by heterogeneous upstream accounts, and a versioned management JSON API (`/management-api/v0/*`). The gateway is deliberately headless: no frontend, no static assets, no accounts, no sessions.

janusgate-web is the missing control plane — the official, statically hosted single-page application through which one operator administers a JanusGate instance entirely over the network. It is the sanctioned UI for the management API, lives in this repository as the `web/` workspace, and is deployed separately from the gateway it administers.

Success for the web UI: an operator can connect to a running gateway and reach a correctly routed, working OpenAI-compatible endpoint — accounts created, models discovered, serving state curated, expired credentials replaced — without hand-editing the configuration file, reading gateway logs, or calling the API themselves.

## Positioning

Gateways that ship an admin UI ship it inside the server: the UI is bound to one deployment, its version, and its origin. JanusGate is headless and its frontend is a separate artifact — **one static build with no build-time or deployment-time configuration administers any instance, anywhere**. The operator proves control by presenting the instance's own management token, verified by a probe against that instance; nothing about the instance is baked into the artifact, and the app has no backend of its own.

Two mechanisms a neighboring product could not truthfully copy: the artifact is instance-agnostic by construction, and the frontend **compiles against TypeScript bindings generated from the gateway's own Rust sources**, so its wire contract cannot drift into a hand-maintained parallel.

The gateway it fronts carries the same posture: strict model passthrough (no automatic mapping, aliasing, or fallback), upstream accounts — ChatGPT and Google OAuth subscriptions as well as static API keys — as first-class identities, and no request content retained (numeric request accounting aside).

## Operating Context

- **The gateway is headless and file-configured.** It runs from a JSONC configuration file (`-c`/`--config`) with hot reload, optional TLS, and logging to stderr. Every fact the UI administers lives in the gateway's in-process SQLite-compatible database (Turso) and is reachable only through the management API.
- **Cross-origin is the normal case.** The app's origin and the gateway's origin differ by design, so the instance's `management_api.cors.allowed_origin` must include the app's origin (or `*`), and `management_api.token` is the credential. Fixing CORS means editing that file and restarting the gateway — the UI cannot do it.
- **Connection is per browser.** The pair `{baseUrl, token}` is entered on first visit, verified by one probe call (`providers:list`), then kept only in the browser's `localStorage`. Returning visits skip setup; a 401 clears the pair and returns to setup. One instance at a time; another instance means forgetting the current connection.
- **Real deployment constraints.** The build output is plain static files, so the host needs an SPA fallback for deep links; HTTPS on both sides is expected because the management token crosses the network; there is no server-side component to configure.
- **Development.** `npm run dev:web` (Vite) drives the app against a real gateway instance; `npm run generate-bindings` (requires cargo) regenerates the wire types from the local gateway sources. Tests run in jsdom; rendering is verified in a browser against a real instance.
- **Gateway non-goals that bound the UI's scope:** no multi-tenancy, no billing or rate limiting, no content logging, no automatic provider fallback, no automatic model mapping, no outbound proxying.

## Capabilities and Constraints

**Administered today**, entirely through `/management-api/v0/*` — every route `POST` with a JSON body, `Authorization: <token>` verbatim (no `Bearer` prefix):

- **Providers** — list with serving state and OAuth expiry; create a static-key account (`openai_response` / `openai_chat_completion` / `gemini_generate_content`; name, model prefix, base URL, API key, enabled); rewrite any provider's complete readable state, with credentials write-only; delete (cascading to its models).
- **OAuth accounts** — start a flow for `openai_oauth` (ChatGPT account) or `antigravity_oauth` (Google account), then paste the returned code to create the provider with its credentials, or to reauthorize an existing one in place.
- **Models** — list (optionally filtered to one provider); discover a provider's models from upstream as a mark-plus-upsert that writes only the row's `discovery_state` (`discovered` / `vanished` / `manual`) — never deleting a row, never rewriting curated columns, and refusing an empty upstream report; edit a row's two writable columns (public ID override, serving switch) addressed by its never-recycled row id; delete a row; create a row by its natural key `(provider_id, upstream_id)`, which also claims a vanished row as the manual recovery path.
- **Serving-API keys** — list, issue (the plaintext secret is returned once, for the administrator to copy), rewrite a key's complete state (rename, rotate, serving switch), and revoke each key individually.
- **Usage reads** — list recorded request-accounting rows newest first with cursor pagination, and aggregate them into buckets by day, model, provider, or API key over a time window with row-id filters; rows and buckets carry ids and numeric facts only — labels resolve through the entity lists.
- **Derived facts the UI states under the gateway's documented definitions:** the three-input effective serving state (`provider enabled && model enabled && not vanished`), derived client-side from rows the app already fetches and never read off the wire; the composed public model ID (`id_override ?? model_prefix + upstream_id`), previewed locally and composed by the gateway at write time; and OAuth credential freshness.
- **Vocabulary fixed by the domain:** provider, model prefix, upstream ID, public ID, discovery and its three states, serving key, request outcome, reauthorization. There is no level vocabulary: reasoning effort is an opaque client string the gateway passes through verbatim.

**Planned, confirmed by the user, mechanism not yet decided:**

- **Observability of gateway state and credential health** — OAuth expiry and refresh state, upstream reachability, discovery results, model counts, serving state. Request-level insight is no longer part of this pending work: it ships through the administered usage reads above, fed by request accounting that retains one numeric row per dispatched request (README §3 *ApiLog*, governed by `api_logs.is_enabled`), while content — prompts, completions, headers, upstream error bodies — remains forbidden by the stateless-execution principle.

**Resolved (2026-09-22):** the first Usage surface is list plus aggregate query — request-detail pagination (`usage:list`, filters by model/provider/key plus time window) alongside bucketed aggregates (`usage:query` by day, model, provider, or API key), labels resolved through the entity lists. Model rows are addressed by their never-recycled row id; the previous `(provider_id, upstream_id)` deep links are not preserved. Discovery state (`discovered` / `vanished` / `manual`) plus the client-derived effective status is the status language; a vanished row states its recovery paths (upstream reappearance or a `models:create` claim).

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
- **Resolved (2026-09-18, shipped):** the previous web UI visual world (the dark route-setting panel) was replaced rather than refined, per the earlier decision. The current `web/DESIGN.md` — the quiet light-first console — is the visual authority; the old look survives nowhere and constrains nothing.

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
