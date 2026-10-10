# Architecture

JanusGate is a single-process, headless AI gateway. The Rust server exposes inference and management APIs; an independently hosted static console consumes only management APIs. One administrator owns the configuration, credentials, and authored provider recipes. There are no user accounts, sessions, cookies, or tenants.

## System boundaries

```text
Inference client → protocol parsing → access and route selection → upstream rendering
                                                                       ↓
Inference client ← protocol rendering ← normalized events ← upstream response

Static console → management API → control-plane state in Turso
                                 → OAuth, discovery, and quota requests
```

The listener serves HTTP/1 and HTTP/2, with optional TLS for the entire listener. `server` owns connections; Axum owns routers and handlers. `/api` contains isolated protocol routers; `/management-api` is mounted only when configured at startup and contains version-owned routers. Unmatched paths outside a version/protocol return an empty 404; requests inside those domains use their own error envelopes. Authentication runs before endpoint execution; CORS preflight is handled before authentication.

The shared serving layer owns HTTP admission, route selection, compaction coordination, delivery, and final accounting. Format codecs own protocol parsing and rendering, including request normalization and replay projection. They do not acquire credentials, select accounts, or make requests. A protocol-neutral IR carries messages, function tools, images, reasoning, usage, and positioned compaction state between codecs.

Provider definitions supply connection data. Generic interpreters execute expressions, OAuth, provisioning, discovery, and quota recipes. Adding an account for an existing wire format needs definition data, not a provider-specific backend branch. Frontend presets are conveniences with recorded provenance; they do not constrain backend capabilities.

## Domain model

| Entity | Responsibility |
| --- | --- |
| Provider definition | Immutable `(name, version)` recipe for one upstream format |
| Provider | Credential-bearing upstream account bound to an exact definition version |
| Physical model | Account-local exact upstream ID, discovery state, serving switch, and capability facts |
| Provider pool | Ordered account membership, model prefix, and independent serving switch |
| Public model route | Stable identity and public ID targeting a physical model or a pool plus exact upstream ID |
| Serving key | Inference secret, serving switch, and sparse model access policy |
| Quota restriction | Durable account-wide or exact-model exhaustion evidence shared across pools |
| Accounting row | Numeric facts for one dispatched physical-account attempt |

[Routing](routing.md) defines availability and access. [Provider definitions](providers.md) define recipes and credential lifecycle. [Storage](storage.md) defines durable identities and accounting. [Compaction](compaction.md) defines client-held context state.

## Execution and ownership

A request authorizes a serving key, parses the client format, authenticates recognizable gateway carriers, and resolves an admitted route from a coherent database snapshot. Target compatibility, account eligibility, optional quota checks, and OAuth renewal precede dispatch. Public IDs stay client-facing; the exact physical upstream ID is rendered to the upstream.

Upstream framing follows the definition's declared streaming mode, never response-header sniffing. Responses become normalized events and render in the caller's format. Streaming clients receive translated SSE; non-streaming delivery aggregates the same events. A response lifecycle owns its unfinalized accounting draft exactly once. Non-streaming aggregation continues after handler departure so usage can finalize; cancellation during streaming closes the body and finalizes the attempt. Internal compaction calls share one outer response and account commitment.

No database transaction spans network I/O. Durable mutations and automatic route synchronization commit atomically. Concurrent refresh and quota work use ownership/fencing so old results cannot overwrite newer account state.

## Global assumptions

- Request content is transient. Prompts, completions, headers, upstream error bodies, and compaction artifacts are never persisted or logged. Affinity stores keyed fingerprints and account metadata only; sealed compaction artifacts live with clients.
- Gateway-defined JSON has concrete types. Arbitrary JSON is limited to externally owned schemas, tool data, template literals, and replay fragments whose shape the gateway cannot define.
- The upstream is authoritative for model and sampling support. Capability declarations inform clients and compaction decisions; ordinary inference is not rejected merely because a declaration is missing or stale.
- Unsupported client semantics are rejected explicitly. Opaque native state is never silently flattened into ordinary text or moved to an incompatible producer.
- Configuration and invariant violations fail fast. A panic in any task terminates the process with exit code 1. Expected client faults, upstream failures, and failed live reloads use their documented error contracts.

The system excludes multi-tenancy, billing/rate limiting, round-robin or weighted balancing, ambiguous-outcome retries, automatic model-name mapping, outbound proxying, and server-side conversation storage.
