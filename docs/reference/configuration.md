# Configuration and deployment

Pass a JSON or JSONC file with `janusgate -c <file>` or `--config <file>`. Unknown fields and invalid typed values fail startup. `{}` is valid: loopback inference with recording enabled, management disabled, and no keys in a fresh database. See the [annotated example](../../config.example.jsonc).

## Schema and defaults

| Field | Shape / default | Effect |
| --- | --- | --- |
| `log_level` | `off`, `error`, `warn`, `info`, `debug`, `trace`; default `info`, case-insensitive | Minimum stderr severity |
| `listen` | IP literal and port; `127.0.0.1:4718` | Listener; DNS names are rejected |
| `tls` | Optional `{certificate: string, private_key: string}` | PEM chain (leaf first) and matching key; absent means HTTP |
| `database` | `{path: string}`; default `janusgate.db` | Main database file |
| `management_api` | Optional `{token: string, cors?: Cors}` | Presence enables management routes; token is required |
| `api` | `{cors?: Cors, logs?: Logs}`; defaulted | Inference CORS and numerical accounting |
| `compaction` | Optional `{keys: [{id: string, key: string}, ...]}` | Gateway carrier sealing/unsealing |
| `pooling` | Positive integer limits below; defaulted | Live quota cadence and affinity bounds |

Relative database, accounting, and TLS paths resolve beside the configuration file, never the process working directory. Database connection-pool sizing is fixed, not configuration.

`Cors` is `{allowed_origin?: string | string[], allow_private_network?: boolean}`. Defaults are `[]` and false. `"*"` or `["*"]` permits any origin; exact origins restrict browser access. Management and inference CORS are independent; they do not replace token/key authentication. Console deployment normally needs its exact origin in management CORS and an SPA fallback on its static host.

`Logs` is `{is_enabled?: boolean, path?: string, retention_days?: positive_integer}`; defaults are true, `api_logs`, and 90. Disabling recording preserves existing usage reads. [Storage](../design/storage.md#files-and-retention) defines daily files and retention triggers.

| Pooling field | Default | Meaning |
| --- | --- | --- |
| `quota_refresh_interval_seconds` | 60 | Automatic quota check cadence |
| `sticky_idle_timeout_seconds` | 86400 | Historical affinity idle lifetime |
| `sticky_retry_timeout_seconds` | 60 | Fresh-request retry hint lifetime |
| `sticky_maximum_entries` | 100000 | Counted affinity entry cap |

Compaction requires a nonempty key array, unique nonempty IDs without `.`, and standard base64 keys decoding to 32 bytes. Generate material with `openssl rand -base64 32`. The first entry seals, all entries unseal by ID. Put a new active key first and retain old entries until their artifacts may be invalidated. No keys means native passthrough only for operations needing no gateway carrier; [compaction](../design/compaction.md) defines failures.

## Reload and restart

The configuration and TLS files are watched with a 500 ms quiet-window debounce. Parse, path resolution, TLS, and sealing keys apply as one generation; failure logs an error and keeps the prior generation. Invalid startup input is fatal.

| Applies live | Applies after restart |
| --- | --- |
| Management token; log level | Listener address |
| TLS presence/material for newly accepted connections | Main database path |
| Accounting switch at dispatch; retention at later sweeps | Accounting directory |
| Pooling limits at their next use | Both CORS policies |
| Compaction key set at its next operation | Presence/absence of management routes |

Existing TLS connections keep serving. Removing management configuration by reload leaves already mounted routes rejecting authorization until restart removes them. Adding the section needs restart to mount them.

## Transport and resource limits

Inbound HTTP/1 and HTTP/2 share one listener; TLS advertises both by ALPN. Upstream clients connect directly and refuse redirects, so authored secrets cannot be resent to a Location destination.

| Boundary | Limit |
| --- | --- |
| Management / inference request body | 2 MiB / 64 MiB |
| Upstream complete success body | 64 MiB |
| Upstream SSE frame | 16 MiB |
| Upstream failure-body read / client excerpt | 4 KiB / 400 characters |
| Upstream connect | 10 seconds |
| Inference read idle gap | 300 seconds; no total-generation deadline |
| Each control request, including body read | 30 seconds |
| HTTP/1 request headers after protocol detection | 30 seconds |
| HTTP/2 idle ping / acknowledgment | 30 / 20 seconds |
| HTTP/2 concurrent streams / decompressed header list | 200 / 16 KiB |

No initial protocol-detection/TLS-handshake deadline is imposed. Provisioning polls have their authored total budgets; each control call still has its own transport timeout.

## Logs and file protection

Logs go to stderr as plain UTC timestamped text. Severity comes only from `log_level`; `RUST_LOG` and per-target directives are not used. Dependencies share the global filter. Gateway logs exclude secrets, OAuth codes/state, request content, compaction artifacts, and upstream error bodies. Internal failures expose fixed client messages and retain safe cause/context in logs.

Use HTTPS for remote clients and console access. Protect configuration, TLS keys, sealing keys, and database files: upstream credentials and serving keys are plaintext. The console stores its management connection in its own origin's localStorage. The gateway offers no backup orchestration; preserve main state and sealing keys, and preserve accounting partitions when their history is needed.
