# Storage and accounting

The main Turso database owns control-plane state. Separate daily Turso files own numerical request accounting. Neither store contains conversation content. Provider credentials, OAuth slots, and serving keys are stored in plaintext; the operator protects their files.

## Durable state and identity

The main database contains immutable provider-definition versions, providers, OAuth flows, physical models, pools and ordered memberships, public routes, quota restrictions, and serving-key policies. [Architecture](architecture.md#domain-model) defines the entities; [routing](routing.md) defines their lifecycle and deletion behavior.

Referenced row identities use `INTEGER PRIMARY KEY AUTOINCREMENT` and are never recycled. A recreated name is a new identity. Accounting attribution uses these IDs without foreign keys or copied labels, so deleting entities preserves historical attribution. All stored timestamps and management timestamps are UTC microseconds; protocol timestamps keep their protocol units. Definitions have positive monotonic versions under a name, and each provider binds an exact immutable version.

Writes that validate and replace related state run atomically, including pool membership, key policies, quota fences, and automatic exposure. No transaction crosses upstream I/O. Dropping an unfinished transaction discards its connection rather than recycling a connection holding a write lock. Database read results are materialized before returning their connection.

## Schema contract

Schema modules under `src/database/schema/` contain hand-written `SCHEMA` constants and are immutable. A change adds the next contiguous version, its complete DDL, migration from the preceding version, and registry entry. Main state and accounting have independent registries and application IDs (`JGMD` and `JGAL`). Tables are `STRICT`; foreign keys remain enabled throughout every migration statement.

A fresh database starts at version 1 and walks forward. Each migration and its `user_version` advance commit together under a write lock, rechecking the current version so concurrent startup cannot apply a step twice. A crash resumes from the last committed version. Unknown versions, foreign application IDs, or failed schema batches yield typed schema drift; startup never deletes or silently recreates a database. Main-file drift is fatal at startup; drift in an accounting partition fails its read or its best-effort write.

The current main and accounting baselines each have version 1. Databases from incompatible unreleased layouts require explicit operator recreation; there is no compatibility reader. SQL constrains storage type, nullability, keys, enums, numeric ranges, JSON validity, and cross-column invariants. Text composition and descriptor semantics belong to typed ingestion/write boundaries, not single-column SQL format checks.

## Request accounting

One row represents one dispatched physical-account attempt. Dispatch begins immediately before upstream `send()`; authentication, routing, quota preflight, credential renewal, and rendering failures before dispatch produce no row. The first dispatch fixes duration origin and the recording switch. Recording disabled does not change account commitment or error behavior.

Rows contain public-route, physical-model, provider and key IDs; endpoint; stream flag; dispatch instant; duration in whole milliseconds; outcome; outward HTTP status; first upstream HTTP status; and five usage columns. Output tokens include reasoning. No prompt, response, credential, header, error body, requested reasoning effort, or compaction artifact is recorded.

Outcomes are `completed`, `client_aborted`, `upstream_error`, and `internal`. A null outward status means no response was produced; null upstream status means no upstream headers arrived. An abandoned quota rejection has its foreign status but no outward status. A 2xx header commits its account even if parsing later fails.

### Finalization and usage

An attempt finalizes exactly once on success, failure, or cancellation. Streaming delivery owns its draft until completion/error/disconnect. Detached non-streaming aggregation continues after handler departure, preserving usage; successful delivery is `completed`, departure before handoff is `client_aborted`, and stream failure keeps its failure classification. A disconnect after non-streaming handoff is unobservable and may remain completed. Streaming failures after response commitment retain outward HTTP 200 and emit protocol error frames.

Compaction stages on one account share an attempt: billed native/summary/answer usage sums once, including already observed usage before a later failure. Counters and estimates measure context and do not invent billed usage. Missing usage remains null; a reported zero is known zero. Response-required zero totals are not evidence for accounting measurements. [Compaction](compaction.md#usage) defines client-visible usage separately.

### Files and retention

Files are `api.logs.path/YYYY-MM-DD.db`, selected by the UTC day of the dispatch timestamp, not finalization time. Missing partitions/directories read as empty and are created only by recording. Existing files remain readable with recording off. A corrupt partition fails a usage read explicitly.

Retention runs only when a new day file is created. It deletes files whose day-end is strictly older than now minus the configured retention, never the newly created file; unrecognized filenames are left alone. With no new file or recording off, retention does not run. Late finalization can recreate an expired partition, and a retention/write race can lose a row in an unlinked file. Accounting and retention failures are logged and swallowed without changing inference responses; accounting is best-effort telemetry, not a billing ledger.

## Usage reads

`usage:list` merges partitions newest-first under its filter. Its exclusive cursor is the last returned `started_at`; no row ID is published. Rows sharing a boundary microsecond may be skipped, and late rows behind an already passed cursor may not appear during pagination.

`usage:query` groups by selected dimensions and merges counts/sums across partitions. Null usage contributes no tokens; all-null totals render zero. Empty grouping returns one total bucket, including a zero bucket for an empty selection. Dimension ordering follows the request for bucket ordering; nulls sort first. Labels resolve through entity lists, including deleted historical identities.

See the [management reference](../reference/management-api.md#usage) for filter and aggregation constraints and the [generated usage schemas](../reference/management-api/usage.md) for all published fields.
