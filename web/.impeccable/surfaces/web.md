---
version: 1
slug: "web"
primary_target: "web"
related_targets: []
---

# Surface brief — janusgate-web, whole-app console

**Mode:** Operate. **Target:** the entire `web/` SPA — connection setup, overview, provider list, provider creation, provider detail, provider edit, models overview, model creation, model detail, model edit, serving-key list, key creation, key detail, key edit, usage listing, usage aggregates, not-found.

**Audience and job.** One operator per instance, holding its single management token, arriving mid-task; plus strangers reaching the official public deployment who have read only the READMEs. Job: read the gateway's health and its recent traffic at a glance, change one thing on the page that owns it, create a provider of any kind through one creation page, sync models, reconnect a dead credential, issue a serving key, read the accounting under a filter, and learn where a client points.

**Action and proof.** Every action states its effect where it is taken — save, sync, reconnect, delete — and reports the gateway's own answer there: the app's title, the gateway's message verbatim, a fix only where certain. Truth the surface must prove: one gateway fronting several separate, expiring upstream accounts, answering three outward API families from one model set, administered one page per job.

**Constraints.** Code-only material: system font stacks, authored inline SVG, no asset or webfont files, no third-party runtime assets. CSS-only motion. WCAG 2.2 AA with measured pairs. SolidJS v2 + TSX + Vite + Tailwind v4; the generated bindings are the wire contract and are never restated; static SPA output needing a host fallback; English. User-pinned register: clean and fresh, simple but not plain, modern and unified, subtle motion that keeps operation fluent.

**Memorable moment.** The overview's quiet confidence: five figures that are also doors, a fourteen-day bar chart of requests drawn from the console's own materials, an attention list reading "Everything looks healthy.", and the three endpoint paths an operator copies to point a client at the gateway.

**Open decisions.** Model detail route encoding settled as `/models/<model_id>`. Theme defaults to light, follows the OS, manual override persists. The usage listing carries its filter in the route — a rolling window plus the shared filter fields — while the aggregates page keeps explicit instants; detail-page usage cards stay per-day. Request-level content stays absent by the gateway's statelessness.

## Direction contract

THESIS: One page, one job — the overview reads health and traffic, provider pages administer providers, model pages administer models, key pages hand out the serving secret, usage pages read the accounting, and rows link to editors rather than unfolding into them. It refuses the category's dark cockpit as well as the previous generation's route-setting panel.

OWN-WORLD: Light-first fresh console (`#f2f5f3` canvas, `#ffffff` surfaces, 1px `#e0e7e3` borders, 16px cards, no shadows) with a dark twin behind the same roles; one teal accent for actions and selection; three semantic hues that only paint statuses and notices, always as icon plus words. One sans family at fixed sizes, mono tabular figures for data, CSS-only motion that means state. A figure that is a door carries the chevron a row carries; a chart is drawn from the same materials — accent bars on the card's ground, one 1px rule as its axis, no gridlines, no legend, no second hue.

STORY: The operator lands on figures they can trust and a fourteen-day trend, opens the one page that owns their task, changes one thing, reads the gateway's answer where they acted, and copies the endpoint path their client needs.

FIRST VIEWPORT: Top bar (Janus mark, theme switch, confirmed disconnect), context bar (Overview / Providers / Models / API keys / Usage tabs, instance address, update stamp, refresh), then the overview: five clickable figures — providers, models routing, API keys, requests and errors over the last 24 hours — the fourteen-day requests chart, the attention card or its all-clear, and the endpoints card. Primary action: New provider.

FORM: Quiet console with settings-style drill-down; the incumbent world extended rather than replaced (seed key `275266d5`). The overview's figures became doors and a trend joined them; the usage listing gained the shared filter, carried by its route.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance
