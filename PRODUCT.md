# JanusGate Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

Operators of self-hosted JanusGate instances, holding that instance's single management token. The same operator may be setting up a gateway for the first time or returning to fix an account, change access, or inspect traffic. The console must work for someone who knows their upstream accounts and client but has not studied management payloads.

## Product Purpose

JanusGate makes heterogeneous AI accounts callable through one gateway and three outward protocol families. Its administration console turns that headless gateway into a working endpoint: connect accounts, inventory models, expose public names, grant serving-key access, and recover credentials. Success means the operator can configure a client with a permitted model, endpoint, and key, then understand why it can or cannot serve.

## Positioning

A lightweight self-hosted gateway with a separate static administration client. The console can connect to different gateway instances by address/token without coupling its hosting to the server. It manages the gateway's existing capabilities; it is not a chat client, inference playground, multi-user organization system, or billing platform.

## Operating Context

The gateway owns configuration, credentials, models, routing, permissions, quota evidence, and numerical accounting. The browser connects directly to its management API across origins. Operators may arrive from a separately hosted console or a local build, on wide or narrow screens, with healthy or expired accounts. Setup requires a running reachable gateway, its token, and an allowed browser origin; the console cannot rewrite the server's configuration or repair network/TLS policy.

## Capabilities and Constraints

- Static-key and OAuth provider creation, reauthorization, versioned definitions, and editable recipe presets.
- Physical model discovery/manual inventory, declared capabilities and overrides, automatic prefix-based exposure, authored aliases, and ordered account pools.
- Serving-key lifecycle and hierarchical model-access policies with explicit permission provenance.
- Stored availability, explicit quota measurement/reset, request-attempt usage, aggregates, and client endpoint handoff.
- English, Simplified/Traditional Chinese, Japanese, Korean, and Spanish; OS-aware themes and browser-local preferences.

One management token controls an instance. There are no operator accounts, roles, cookies, or server sessions. The console stores that token as plaintext in browser preferences when possible; gateway credentials and serving secrets are also sensitive operational data. Usage contains numerical metadata, not prompt/response content. Quota reset clears gateway evidence, never upstream quota. There is no general account failover after accepted work.

## Brand Commitments

Plain, direct, and calm. Use established domain words and make the consequence of an action clear. Present known state precisely, acknowledge unknown state, and show gateway messages without inventing a diagnosis. Reduce the work needed to reach a functioning endpoint without obscuring identity, permission, or irreversible deletion effects.

## Evidence on Hand

The [gateway contracts](docs/README.md), [management schemas](docs/reference/management-api/README.md), and [console architecture](web/docs/architecture.md) describe implemented behavior. The repository contains a working browser client and its typed API integration. These sources establish capability and constraints; no usability study or external market claim is required to interpret them.

## Product Principles

- Lead setup from an account to discovered models and a permitted serving key. Automatic exposure makes alias creation optional.
- Give each page a clear job and state ownership. Protect unsaved work across refresh, navigation, and language changes.
- Keep physical availability, public exposure, and key permission distinct. Unknown capability or usage is not false or zero.
- Explain an operation's result where the operator performed it, with an actionable recovery only when known.
- Preserve stable identity across routine edits and reauthorization; expose deletion consequences before commitment.

## Accessibility & Inclusion

Keyboard operation, visible focus, labelled fields, readable contrast, and status expressed in words as well as color are requirements. Respect reduced motion and allow narrow layouts without losing operations. Localize authored copy consistently, keep foreign values intact, and isolate bidirectional machine text. Accessibility is a continuing implementation obligation, not a certification claim. [DESIGN.md](web/DESIGN.md) supplies visual application rules.
