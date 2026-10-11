# JanusGate

JanusGate is a lightweight, self-hosted AI gateway written in Rust. Connect upstream accounts once, then use their models through OpenAI-compatible, Gemini, or Anthropic APIs. A separate static administration console manages accounts, models, account pools, serving keys, and request usage.

## What it does

- Translates between five upstream formats: OpenAI Responses, OpenAI Chat Completions, Gemini GenerateContent, Gemini Interactions, and Anthropic Messages. Every client API can use every upstream format within the [supported feature set](docs/reference/inference.md).
- Supports static API keys and definition-driven OAuth, including authorization-code and device flows. The console includes provider presets; the gateway accepts connection recipes without provider-specific branches.
- Publishes discovered models automatically under configurable prefixes. Optional aliases expose direct models or ordered account pools, with model access controlled per serving key.
- Keeps pooled conversation history on its established account. Account switching is limited to positively identified quota exhaustion before acceptance; ambiguous failures are never retried on another account.
- Provides client-directed remote compaction, using compatible native operations or a gateway-generated summary on the selected account and model.
- Persists account recipes, credentials, routing, and key policies in an in-process SQLite-compatible database (Turso). Request accounting retains identifiers and numerical usage, never prompts, completions, headers, or upstream error bodies.

The gateway is headless: it does not serve the console or other static files. It has no multi-tenancy, billing, rate limiting, outbound proxy, server-side conversation store, or automatic upstream model-name mapping.

## Get started

### 1. Build and run the gateway

A Rust toolchain supporting the repository's Rust 2024 edition and locked dependencies is required. Run from the repository root:

```bash
cargo build --release --locked
```

Create `config.jsonc` with a management token and the origin from which you will open the console:

```json
{
  "management_api": {
    "token": "replace-with-your-management-secret",
    "cors": { "allowed_origin": "http://localhost:56625" }
  }
}
```

```bash
./target/release/janusgate --config config.jsonc
```

This listens at `http://127.0.0.1:4718`. Database and accounting paths resolve beside the configuration file. The management section is necessary to configure a fresh instance; an empty configuration is valid but exposes no administration routes and has no serving keys.

For remote deployment, configure the listening address, HTTPS, and the console's actual origin. See the [configuration reference](docs/reference/configuration.md) and [annotated example](config.example.jsonc); the example contains placeholder TLS paths and secrets that require replacement.

### 2. Open the administration console

Build or run the console independently; it requires Node.js and npm in addition to the Rust toolchain used for bindings generation:

```bash
npm ci
npm run generate-bindings -w web
npm run dev:web
```

Open `http://localhost:56625`, enter `http://127.0.0.1:4718` and the management token, then connect. A different hostname is a different browser origin: keep the configured CORS origin aligned with the address you open.

For static hosting, run `npm run build:web` and serve `target/web/dist/` with an `index.html` fallback for client routes. The same build can administer any reachable instance; no instance URL or token is baked into it. See the [console guide](web/README.md).

### 3. Add an account and models

1. Create a provider using a console preset or an authored provider definition. Complete OAuth when the selected definition requires it.
2. Discover its models, or add exact upstream IDs manually when discovery is unavailable.
3. Choose a provider model prefix. Standalone models are automatically exposed as `model_prefix + upstream_id`; pool membership replaces standalone exposure with each pool's own prefix.
4. Optionally create aliases or pools. Copy an enabled public model ID from Public routes.
5. Create an enabled serving key and configure its model access. A new console draft starts with **Deny**; grant the intended routes or owners, or explicitly choose **Allow** as the key default.

The management token administers the instance. A serving key authorizes inference clients; the two credentials have different roles and header formats.

### 4. Point a client at JanusGate

| Client API | Base URL | Authentication |
| --- | --- | --- |
| OpenAI-compatible | `http://127.0.0.1:4718/api/openai-v1` | `Authorization: Bearer <serving-key>` |
| Gemini | `http://127.0.0.1:4718/api/gemini-v1beta` | `x-goog-api-key: <serving-key>` |
| Anthropic | `http://127.0.0.1:4718/api/anthropic-v1` | `x-api-key: <serving-key>` and an `anthropic-version` header |

Set the client's model to the public ID, not an account-local name guessed from the upstream:

```bash
curl http://127.0.0.1:4718/api/openai-v1/chat/completions \
  -H 'Authorization: Bearer YOUR_SERVING_KEY' \
  -H 'Content-Type: application/json' \
  -d '{"model":"YOUR_PUBLIC_MODEL_ID","messages":[{"role":"user","content":"Hello"}]}'
```

Clients must send their conversation history on each request. Unknown fields and unsupported semantic controls are rejected; protocol compatibility does not imply support for every vendor feature. The [inference reference](docs/reference/inference.md) defines endpoints, supported controls, replay, streaming, and errors. The [API administration guide](docs/guides/administration.md) shows setup without the console.

## Deployment assumptions

One administrator controls each instance through a shared management token. Credentials and serving-key secrets are stored in plaintext in the database; protect the configuration and database files and use HTTPS for remote access. The console remembers its connection in browser-local storage, so use a trusted console origin and browser.

Numeric request accounting is enabled by default with 90-day retention. Disable new recording with `api.logs.is_enabled: false`; existing records remain readable. Compaction summaries and wrapped native state require configured sealing keys; native passthrough can work without them. Pool affinity is bounded, process-local, and lost on restart.

## Documentation and development

[Documentation index](docs/README.md) is the map of the project's normative design, assumptions, and public contracts. Start there for architecture, routing, provider recipes, storage, compaction, configuration, and the complete management API method/path/payload reference.

[Development guide](docs/development.md) covers setup, generated references, and quality gates. [AGENTS.md](AGENTS.md) supplies repository working rules and task-specific reading instructions; [web/AGENTS.md](web/AGENTS.md) adds frontend rules. Product context is maintained in [PRODUCT.md](PRODUCT.md), and the console's visual system in [web/DESIGN.md](web/DESIGN.md).

## License

JanusGate is licensed under the [Apache License, Version 2.0](LICENSE). Third-party dependencies retain their own licenses.
