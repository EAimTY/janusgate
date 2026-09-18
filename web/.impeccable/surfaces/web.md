---
version: 1
slug: "web"
primary_target: "web"
related_targets: []
---

# Surface brief — janusgate-web, whole-app replacement

**Mode:** Operate. **Target:** the entire `web/` SPA — connection setup, overview, provider list, provider creation, provider detail, models overview, model detail, not-found.

**Audience and job.** One operator per instance, holding its single management token, arriving mid-task; plus strangers reaching the official public deployment who have read only the READMEs. Job: see the gateway's health at a glance, change one thing on the page that owns it, create a provider of any kind through one creation page, sync models, reconnect a dead credential.

**Action and proof.** Every action states its effect where it is taken — save, sync, reconnect, delete — and reports the gateway's own answer there: the app's title, the gateway's message verbatim, a fix only where certain. Truth the surface must prove: one gateway fronting several separate, expiring upstream accounts, administered one page per job.

**Constraints.** Code-only material: system font stacks, authored inline SVG, no asset or webfont files, no third-party runtime assets. CSS-only motion. WCAG 2.2 AA with measured pairs. SolidJS v2 + TSX + Vite + Tailwind v4; the generated bindings are the wire contract and are never restated; static SPA output needing a host fallback; English. User-pinned register: clean and fresh, simple but not plain, modern and unified, subtle motion that keeps operation fluent.

**Memorable moment.** The overview's quiet confidence: four counts, an empty attention list reading "Everything looks healthy.", and a fresh gateway's four checked-off steps to a working endpoint.

**Open decisions.** Model detail route encoding settled as `/models/:providerId/:encodedUpstreamId`. Theme defaults to light, follows the OS, manual override persists. Request-level figures stay absent until the gateway retains per-request data.

## Direction contract

THESIS: One page, one job — the overview reads health, provider pages administer providers, model pages administer models, and rows link to editors rather than unfolding into them. It refuses the category's dark cockpit as well as the previous generation's route-setting panel.

OWN-WORLD: Light-first fresh console (`#f2f5f3` canvas, `#ffffff` surfaces, 1px `#e0e7e3` borders, 16px cards, no shadows) with a dark twin behind the same roles; one teal accent for actions and selection; three semantic hues that only paint statuses and notices, always as icon plus words. One sans family at fixed sizes, mono tabular figures for data, CSS-only motion that means state.

STORY: The operator lands on counts they can trust, opens the one page that owns their task, changes one thing, and reads the gateway's answer where they acted — never wondering which of several states the screen reports.

FIRST VIEWPORT: Top bar (Janus mark, theme switch, confirmed disconnect), context bar (Overview / Providers / Models tabs, instance address, update stamp, refresh), then the overview: four stat cards, the attention card or its all-clear, and on a fresh gateway the four-step getting-started card. Primary action: New provider.

FORM: Quiet console with settings-style drill-down; my grounded candidate 1, taken as IMPECCABLE'S PICK over the assigned switch-wall grammar (ruled out: a single-control grammar cannot carry tables, forms, and flows) and six declined expressive challengers, each donating one discipline (status wall without flip motion; conflict rows marked in pattern plus words; tabular figures; right-aligned time columns; brightness-layered states; directional state transitions); seed key `275266d5`.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance
