# Documentation

This documentation tree, entered through the root [README](../README.md), is JanusGate's source of truth for current design and behavior. It describes the shipped system, not task history or future proposals. Read the overview first, then the documents relevant to the operation or change.

## Use and operate

| Document | Owns |
| --- | --- |
| [Getting started](../README.md#get-started) | Build, first connection, accounts, models, and serving clients |
| [Administration](guides/administration.md) | Account setup, model access, and recovery |
| [Configuration](reference/configuration.md) | Configuration shape, defaults, reload, deployment, and limits |
| [Console guide](../web/README.md) | Running and hosting the static administration app |

## Design and contracts

| Document | Owns |
| --- | --- |
| [Architecture](design/architecture.md) | System boundaries, execution flow, and global assumptions |
| [Routing and access](design/routing.md) | Model lifecycle, public exposure, permissions, pools, quota, and affinity |
| [Provider definitions](design/providers.md) | Connection expressions, credentials, OAuth, discovery, and quota extraction |
| [Storage and accounting](design/storage.md) | Persistence, identity, migration, retention, and usage semantics |
| [Remote compaction](design/compaction.md) | Controls, native selection, replacement boundaries, replay, and failure behavior |
| [Inference API](reference/inference.md) | Public endpoints, supported request controls, translated responses, and errors |
| [Management API](reference/management-api.md) | Transport, resource semantics, and complete generated payload schemas |
| [Console architecture](../web/docs/architecture.md) | Browser connection, routes, state, editing, and security |
| [Localization](../web/docs/localization.md) | Catalogs, language activation, formatting, and extension workflow |

## Authority and maintenance

Behavioral requirements belong in the owning design or reference document. Exact management methods, paths, and payload declarations are derived from concrete Rust route/type declarations into [resource schema pages](reference/management-api/README.md); regenerate them rather than editing them. Generated schemas state field shapes, while the management reference supplies constraints, defaults, and meanings that types cannot express. Neither a schema nor an example silently replaces a behavioral contract.

[PRODUCT.md](../PRODUCT.md) owns durable audience, purpose, positioning, and product constraints. [DESIGN.md](../web/DESIGN.md) owns visual tokens and reusable visual guidance in the Impeccable format. The [console surface brief](../web/.impeccable/surfaces/web.md) owns its Operate mode and surface strategy. The design sidecar is a tool-readable projection of the visual document, not an independent specification.

[Repository working rules](../AGENTS.md) and folder-local rules instruct contributors and agents how to work. They point to contracts instead of duplicating them. [Development](development.md) owns commands and regeneration workflows.

Update the owning contract alongside a behavior change, regenerate interface references when their source changes, and repair incoming links when moving documentation. Keep examples consistent with defaults and omission/null semantics. Record only important boundaries and externally meaningful assumptions; private algorithm details stay in code and rustdoc. A conflict between code and contract is a defect to resolve explicitly, not permission to guess a new behavior. Temporary plans and review reports are not normative documentation and must not be linked as contracts.
