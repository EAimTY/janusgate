---
name: JanusGate Console
description: A quiet, fresh console where each page does one job, every state reads as icon plus words, and the gateway's answer is stated where the action was taken.
colors:
  # Light (default): faint green-tinted neutrals, one teal accent
  canvas: "#f2f5f3"
  surface: "#ffffff"
  muted: "#e9efec"
  hover: "#eef3f0"
  ink: "#17211d"
  ink-dim: "#4c5a54"
  ink-faint: "#5f6c66"
  rule: "#e0e7e3"
  rule-strong: "#c3cec8"
  accent: "#0b6e5c"
  accent-strong: "#0a6352"
  accent-soft: "#ddf0e9"
  accent-ink: "#ffffff"
  ok: "#147339"
  ok-soft: "#e2f4e7"
  warn: "#9f4c08"
  warn-soft: "#faeedb"
  bad: "#c92a1e"
  bad-soft: "#fdecea"
  bad-ink: "#ffffff"
  scroll-thumb: "#c2cdc7"
  scroll-thumb-hover: "#a9b6b0"
  # Dark: the same roles, re-inked
  dark-canvas: "#0d1412"
  dark-surface: "#151e1b"
  dark-muted: "#1d2925"
  dark-hover: "#223029"
  dark-ink: "#e7eeea"
  dark-ink-dim: "#a9b6b0"
  dark-ink-faint: "#7f8c86"
  dark-rule: "#23312c"
  dark-rule-strong: "#35463f"
  dark-accent: "#3ecfa4"
  dark-accent-strong: "#6fdcb9"
  dark-accent-soft: "#123b30"
  dark-accent-ink: "#05281e"
  dark-ok: "#4cc38a"
  dark-ok-soft: "#123825"
  dark-warn: "#e8a04c"
  dark-warn-soft: "#3d2c12"
  dark-bad: "#f26d5f"
  dark-bad-soft: "#431b16"
  dark-bad-ink: "#1a0b09"
  dark-scroll-thumb: "#33443c"
  dark-scroll-thumb-hover: "#43564d"
typography:
  family: 'ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, "PingFang SC", "Hiragino Sans GB", "Microsoft YaHei", sans-serif'
  data: 'ui-monospace, SFMono-Regular, Menlo, Consolas, "Liberation Mono", monospace with tabular-nums'
  page-title: '20px, 600, tight tracking'
  section-title: '15px, 600, tight tracking'
  body: '14px, 400, 1.55'
  small: '13px body copy, 12.5px captions'
rounded:
  card: "16px"
  control: "8-12px"
  pill: "999px for badges and switches"
motion:
  grammar: "CSS only. 150-200ms ease-out on interactive state; one 200ms rise-in per screen entrance; skeleton wash while the first read is in flight. Nothing else moves."
  reduced: "prefers-reduced-motion parks every animation; transitions collapse to near-zero."
components:
  button-primary: "accent fill, white text"
  button-secondary: "surface, strong-rule border, ink text"
  button-danger: "bad fill, bad-ink text"
  button-ghost: "transparent, dim text"
  badge: "soft tint ground, icon plus words, pill"
  notice: "soft tint ground, thin tinted border, icon plus title plus the gateway's verbatim detail"
  card: "surface, 1px rule border, 16px radius, no shadow"
  field: "canvas ground, strong-rule border, accent focus, 8px radius"
  switch: "pill track with sliding knob, role=switch, on/off text beside it"
  check-pill: "a checkbox in a pill frame for a closed-set pick, accent edge and soft tint while checked"
  stat-link: "a figure card whose whole surface is an anchor, chevron at its right edge"
  bar-chart: "accent bars on the card ground, one 1px rule as the axis, no gridlines, no legend, a text name per bar"
---

# Design System: JanusGate Console

## Overview

**Creative North Star: "One page, one job."**

The interface is a quiet console for a working operator, not an instrument
to admire. Each page does exactly one thing — the overview reads the
gateway's health and its recent traffic, the provider pages administer
providers, the model pages administer models, the key pages hand out the
serving secret, the usage pages read the accounting — and rows never unfold
into editors: a row is one link to that thing's detail page, behind which its
edit page owns every change. The operator always knows where they are, what
this page owns, and where the one action they came for lives.

The world is built from restraint, not decoration. One neutral family
tinted faintly green, one teal accent for actions and current selection, and
three semantic hues that only ever paint a status or a notice. Surfaces are
flat white (or flat ink in the dark theme) separated by 1px borders; there
are no shadows, no gradients standing in for content, no display face, no
webfont, no image file, no icon font. Type is the platform's own stacks —
one sans for everything, the mono stack with tabular figures for anything
that identifies a row: IDs, prefixes, counts, timestamps. Status is always
icon plus words, never color alone.

The console refuses the category's two defaults: the dark hacker cockpit
with neon state lamps, and the dense everything-table that edits inline. It
also refuses the previous generation of this same app — the enamelled
route-setting panel with its tracks, sections, levers, and lamps — which is
kept nowhere and referenced by nothing.

**Key Characteristics:**

- Light by default, dark on demand: the OS decides unless the operator
  overrode it; the override persists beside the connection, not in a build.
- One page, one job; rows link to detail pages, edit pages own every change.
- Flat surfaces, 1px borders, 16px cards, pill badges, no shadows.
- Single sans family, fixed sizes; mono tabular figures for data.
- Motion is CSS only and means state: switch flips, notices land, screens
  settle once. Everything parks under `prefers-reduced-motion`.
- Plain English copy. Domain words are provider, model, public ID, upstream
  ID, prefix, enabled, serving, discover, reconnect, override, reasoning
  level, capability, declaration — nothing else needs a glossary.

## Colors

Forty-four tokens: twenty color roles themed twice, plus the scrollbar's
scroll-thumb pair in both themes — the themed browser surfaces, tokenized.
Every color the app paints comes from this set; the only literals elsewhere
are the authored SVG geometry.

### Primary

- **Accent** (light `#0b6e5c`, dark `#3ecfa4`): the one action color.
  Primary buttons, the current nav tab, text links, focus rings, text
  selection, the caret. It never paints a status and never decorates.
- **Accent soft** (light `#ddf0e9`, dark `#123b30`): the tint behind the
  current selection and the info-adjacent fills (empty-state medallions,
  step numerals, kind picker).
- **Accent strong** (light `#0a6352`, dark `#6fdcb9`): the hover fill for
  primary buttons, and the text ink on dark-theme tint grounds.

### Secondary

- **Ok** (light `#147339` on `#e2f4e7`, dark `#4cc38a` on `#123825`):
  serving, valid credentials, success notices. Always with a check mark.
- **Warn** (light `#9f4c08` on `#faeedb`, dark `#e8a04c` on `#3d2c12`):
  expired or stale credentials, suspended and held-by-provider models, the
  usage stream's client-aborted outcomes, unsaved-changes stamps. Always
  with an alert mark or plain words.
- **Bad** (light `#c92a1e` on `#fdecea`, dark `#f26d5f` on `#431b16`):
  failures, destructive actions. Always with a mark — the alert triangle on
  a notice, the cross on a status badge, so a failed request reads apart
  from the alert-marked credential warning. Danger buttons set their label
  in **bad ink** (light `#ffffff`, dark `#1a0b09`).

### Neutral

- **Canvas** (light `#f2f5f3`, dark `#0d1412`): the app ground, and the
  ground inputs sit on so their boundary reads on a card.
- **Surface** (light `#ffffff`, dark `#151e1b`): cards, rows, the nav bar.
- **Muted** (light `#e9efec`, dark `#1d2925`): quiet fills — skeletons,
  search-count pills, dark-theme input grounds.
- **Ink / Dim / Faint**: the three text inks. Faint carries hints, counts,
  captions, and placeholders — all of which still clear 4.5:1.
- **Rule / Strong rule**: 1px borders; strong rule bounds inputs and
  secondary buttons.

### Contrast

Measured pairs (WCAG 2.x relative luminance).

Light, on surface `#ffffff` unless noted: ink 16.5, dim 7.3, faint 5.5
(5.0 on canvas), accent 6.2, white on accent 6.2, accent on soft 5.2,
ok 5.2, warn 5.2, bad 4.8, bad-ink on danger fill 4.8.

Dark, on surface `#151e1b`: ink 14.4, dim 8.1, faint 4.9, accent 8.7,
accent-strong on soft 7.5, dark text on accent fill 8.0, ok 5.9,
warn 6.1, bad 5.1, bad-ink on bad fill 6.5.

**The Badge Rule.** A status is a pill carrying an icon and words on a
soft tint. A badge that is only a colored dot does not exist.

**The Notice Rule.** A result is stated where its action was taken: the
app's own title, the gateway's message verbatim as the detail, and a fix
only where the app can be certain of one. Failures are `role="alert"`.

## Typography

**One family for everything.** The platform sans carries titles, labels,
buttons, and body; there is no display face and no pairing to maintain.
Sizes are fixed, not fluid, each painted where it belongs: 26px stat
figures, 22px the connection screen's title, 20px page titles, 16px
empty-state titles, 15px card titles, 14.5px table row names, 13.5px
controls, 13px supporting copy, 12.5px captions, 12px step numerals — the
14px body base is set once by the stylesheet, and no utility re-states it.
Titles track tight; nothing is ever set in all-caps as a voice.

**Data is mono with tabular figures.** Anything that identifies a row —
public and upstream IDs, prefixes, provider addresses, states, expiries,
counts — is set in the mono stack at 0.92em with `tabular-nums`, so
columns of figures align. Mono is measurement, never a costume: prose
about the data stays in the sans.

## Components

- **Buttons** come in four tones — primary, secondary, danger, ghost —
  one shape, one rhythm. Labels name the action ("Discover models", "Save
  changes", "Delete for good"); busy states name the wait ("Discovering…").
- **Fields** are labelled above, hinted below, errored below in bad with
  an alert mark. Inputs sit on the canvas ground with a strong-rule edge;
  focus turns the edge accent.
- **Switches** are pills with knobs, `role="switch"`, with an On/Off word
  beside them. A switch flips an enabled state — always labelled `Enabled`
  with no caption, always first in its settings — it never submits a form.
- **Cards** hold one subject: details, settings, credentials, models, usage,
  danger. A card has a title, an optional one-line description, and the
  actions that belong to it — nothing else.
- **Stat links** are the overview's figures: a card whose whole surface is an
  anchor, the figure in tabular figures at 26px, its name under it, and the
  chevron a row carries at its right edge. Hover strengthens the border, lifts
  the ground, and turns the chevron accent — a figure that goes somewhere
  never looks like a figure that does not.
- **Charts** are drawn from the console's own materials: one series, accent
  bars on the card's ground, a single 1px rule as the axis, and no gridlines,
  no legend, no second hue. A quiet day is a 2px tick at the axis, never a
  gap. Every bar is a focusable point whose accessible name is its day and
  figure; the axis prints the window's two ends, and a readout line states
  the window's total and peak until a bar is read, then that day and its
  figure — no figure is readable only by hovering.
- **Skeletons** hold the layout while a screen's first read is in flight.
  A failed first read replaces the skeleton with the failure, never stacks
  under it.
- **Empty states** teach: what this is, and the one way forward. The
  gateway-wide empty state carries the five steps from here to a working
  endpoint, with the finished steps checked off.
- **Armed confirms** replace modals: Delete… arms, "Delete for good" plus
  "Keep it" decide, and the blast radius (cascade count, 404 warning) is
  stated on the armed state.

## Motion

One grammar, four members, all CSS: interactive state at 150–200ms
ease-out, the screen's one 200ms rise-in entrance, the skeleton wash, and
the switch knob's travel. Page entrances never orchestrate children;
results never slide across the screen; tables never animate rows. Motion
conveys state — a flip, a landing, a settling — and nothing else.

## Copy

Controls name their action; errors name the problem and the recovery; the
gateway's message is always kept verbatim as the detail. The fixed
vocabulary: provider, model, public ID, upstream ID, prefix, enabled,
serving, discover, reconnect, override, reasoning level, capability,
declaration. Setup copy assumes a
stranger who read only the READMEs; in-app copy assumes an operator mid-task.

## What this world refuses

- Inline row editors, expandable table rows, and modals for ordinary
  confirmations.
- State carried by color alone; colored edge strips; gradient text; glass.
- Kickers and eyebrows above headings; section numbers; hero-metric
  stat blocks (figures here are labelled stats and doors, not decoration).
- Display faces, webfonts, image assets, icon fonts, emoji-as-icons.
- Anything that moves beyond the motion grammar, and any motion that
  needs JavaScript to run.


Standalone provider models are automatically callable as `provider.model_prefix + upstream_id`. Any pool membership hides these entries; each pool automatically exposes the union of member upstream IDs as `pool.model_prefix + upstream_id`, selecting only members with that exact model. Provider and pool identity forms expose editable prefixes, including the empty prefix, and explain the renaming effect beside the field. Inventory rows and physical model details show the actual automatic names. Public-route listings distinguish automatic entries and link them to the owning physical model or pool. Additional direct aliases and pool routes remain optional; setup leads from discovery directly to issuing a serving key.
