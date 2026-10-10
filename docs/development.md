# Development

Read the [project introduction](../README.md), then the [documentation map](README.md) and the contract for the area you will change. [AGENTS.md](../AGENTS.md) contains repository-wide authoring rules; [web rules](../web/AGENTS.md) and [management rules](../src/management_api/AGENTS.md) apply in their directories.

## Workspace and commands

Rust owns the gateway, persisted data, protocol translation, and management API. The `web/` npm workspace owns a separately deployed SolidJS SPA. Use a Rust toolchain satisfying `Cargo.toml` and Node/npm versions satisfying the locked frontend dependencies. Both dependency lockfiles are committed.

```bash
cargo build --locked
npm ci
npm run generate-bindings -w web
npm run dev:web
```

The bindings generator needs Rust and writes ignored files under `web/src/management-api/generated/`. Run it before frontend typechecking or building a fresh checkout, and after changing a management payload. The normal gateway build does not enable the bindings feature.

| Purpose | Repository-root command |
| --- | --- |
| Gateway development | `cargo run --locked -- --config config.dev.jsonc` |
| Frontend development | `npm run dev:web` |
| Frontend production build | `npm run build:web` |
| Frontend quality gate | `npm run check` |
| Regenerate management reference | `npm run docs:generate` |
| Check management reference against Rust | `npm run docs:check` |

The frontend development server listens on port 56625. Production assets land in `target/web/dist/`; the gateway does not serve them. The development configuration and build products are ignored. Never commit live credentials.

## Changes and verification

Write functionality tests before implementation. Specify observable behavior and failure boundaries in the owning document alongside the change. Keep volatile implementation details in code/Rustdoc; keep stable cross-module contracts, assumptions, and interfaces in the documentation hierarchy.

After Rust changes, run the complete required matrix:

```bash
cargo fmt --all
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets
cargo clippy --workspace --all-targets --all-features
```

Formatting is feature-independent; its second invocation verifies the result. After web code changes, regenerate bindings if necessary and run `npm run check`. Build the SPA when changing build behavior, dynamic loading, or deployment assumptions. Run relevant functionality tests for the behavior changed; formatting and static checks do not replace them.

After management changes, regenerate the committed [schema reference](reference/management-api/README.md), review semantic changes in [the management contract](reference/management-api.md), and regenerate frontend bindings. `npm run docs:check` generates into a temporary directory and compares without rewriting documentation. The Rust route registry owns methods and paths; serde-aware declarations own payload shapes; the human-written contract owns behavior, bounds, and defaults. Do not maintain another schema inventory by hand.

## Documentation ownership

[README.md](../README.md) is the human entry point and source-of-truth navigation. [Design documents](README.md#design-and-contracts) define the implemented system; [references](README.md#use-and-operate) define external contracts. [PRODUCT.md](../PRODUCT.md) owns product intent, while [web/DESIGN.md](../web/DESIGN.md) owns visual tokens and design rules. A decision belongs in one authoritative location; other pages link to it.

Keep `AGENTS.md` focused on stable working guidance and area-specific reading order. Task plans, reviews, measurements from one run, and conversations are temporary work artifacts, not product specifications. Impeccable's durable web design sidecar and surface brief are committed alongside their owning documents; transient `.impeccable` artifacts remain ignored. Installed skills are third-party tooling; their lockfile records the installation and their instructions are not project architecture.
