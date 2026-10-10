---
name: "JanusGate Console"
description: "A quiet administration console: one page, one job."
colors:
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
  body:
    fontFamily: "ui-sans-serif, system-ui, -apple-system, \"Segoe UI\", Roboto, \"Helvetica Neue\", Arial, \"PingFang SC\", \"Hiragino Sans GB\", \"Microsoft YaHei\", \"Noto Sans CJK SC\", \"Geeza Pro\", \"Noto Sans Arabic\", Tahoma, sans-serif"
    fontSize: "14px"
    fontWeight: 400
    lineHeight: 1.55
  page-title:
    fontFamily: "ui-sans-serif, system-ui, -apple-system, \"Segoe UI\", Roboto, \"Helvetica Neue\", Arial, \"PingFang SC\", \"Hiragino Sans GB\", \"Microsoft YaHei\", \"Noto Sans CJK SC\", \"Geeza Pro\", \"Noto Sans Arabic\", Tahoma, sans-serif"
    fontSize: "20px"
    fontWeight: 600
    letterSpacing: "-0.025em"
  section-title:
    fontFamily: "ui-sans-serif, system-ui, -apple-system, \"Segoe UI\", Roboto, \"Helvetica Neue\", Arial, \"PingFang SC\", \"Hiragino Sans GB\", \"Microsoft YaHei\", \"Noto Sans CJK SC\", \"Geeza Pro\", \"Noto Sans Arabic\", Tahoma, sans-serif"
    fontSize: "15px"
    fontWeight: 600
  control:
    fontFamily: "ui-sans-serif, system-ui, -apple-system, \"Segoe UI\", Roboto, \"Helvetica Neue\", Arial, \"PingFang SC\", \"Hiragino Sans GB\", \"Microsoft YaHei\", \"Noto Sans CJK SC\", \"Geeza Pro\", \"Noto Sans Arabic\", Tahoma, sans-serif"
    fontSize: "13.5px"
    fontWeight: 500
  caption:
    fontFamily: "ui-sans-serif, system-ui, -apple-system, \"Segoe UI\", Roboto, \"Helvetica Neue\", Arial, \"PingFang SC\", \"Hiragino Sans GB\", \"Microsoft YaHei\", \"Noto Sans CJK SC\", \"Geeza Pro\", \"Noto Sans Arabic\", Tahoma, sans-serif"
    fontSize: "12.5px"
    fontWeight: 400
  data:
    fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, \"Liberation Mono\", monospace"
    fontSize: "0.92em"
    lineHeight: 1.4
    letterSpacing: "-0.01em"
    fontFeature: "\"tnum\""
rounded:
  control: "8px"
  notice: "12px"
  card: "16px"
  pill: "999px"
spacing:
  tight: "8px"
  related: "12px"
  group: "16px"
  card: "20px"
  section: "24px"
components:
  button-primary:
    backgroundColor: "{colors.accent}"
    textColor: "{colors.accent-ink}"
    typography: "{typography.control}"
    rounded: "{rounded.control}"
    padding: "8px 14px"
  button-primary-hover:
    backgroundColor: "{colors.accent-strong}"
  button-secondary:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    typography: "{typography.control}"
    rounded: "{rounded.control}"
    padding: "8px 14px"
  button-danger:
    backgroundColor: "{colors.bad}"
    textColor: "{colors.bad-ink}"
    typography: "{typography.control}"
    rounded: "{rounded.control}"
    padding: "8px 14px"
  button-ghost:
    backgroundColor: "transparent"
    textColor: "{colors.ink-dim}"
    typography: "{typography.control}"
    rounded: "{rounded.control}"
    padding: "8px 14px"
  field:
    backgroundColor: "{colors.canvas}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    padding: "8px 12px"
  card:
    backgroundColor: "{colors.surface}"
    textColor: "{colors.ink}"
    rounded: "{rounded.card}"
    padding: "20px"
  badge:
    backgroundColor: "{colors.ok-soft}"
    textColor: "{colors.ok}"
    rounded: "{rounded.pill}"
    padding: "4px 10px"
  notice:
    backgroundColor: "{colors.bad-soft}"
    textColor: "{colors.ink}"
    rounded: "{rounded.notice}"
    padding: "12px 14px"
  switch:
    backgroundColor: "{colors.accent}"
    rounded: "{rounded.pill}"
    height: "24px"
    width: "44px"
  nav:
    backgroundColor: "{colors.accent-soft}"
    textColor: "{colors.accent}"
    rounded: "{rounded.control}"
---

# Design System: JanusGate Console

## Overview

**Creative North Star: "One page, one job."**

The console gives an operator one clear job per page. Overview reads health and traffic; lists lead to a resource; its detail page explains state; its editor owns changes. Pools and public routes use dedicated editors. Physical availability, public exposure, and serving-key permission remain distinct.

The visual system is quiet and flat: green-tinted neutrals, one teal action accent, and semantic status hues. System fonts and inline SVG supply the materials; there are no remote visual assets. Light and dark themes share roles and follow the OS unless overridden. Motion conveys a state change, then stops.

**Key Characteristics:**

- One page, one job; explicit resource and action ownership.
- Flat bordered surfaces, compact controls, and generous grouping.
- One sans stack for copy; mono tabular figures for machine values.
- Status always includes words and an icon.
- Localized copy, narrow layouts, visible focus, and reduced motion.

## Colors

The frontmatter lists the implemented CSS roles and their dark equivalents. `canvas` is the app ground; `surface` is a card or row; `muted` and `hover` provide quiet state fills. `ink`, `ink-dim`, and `ink-faint` establish text hierarchy. `rule` separates surfaces; `rule-strong` bounds controls. Scrollbar colors belong to this same role system.

`accent` marks primary actions, focus, links, and selection, with `accent-strong` for interaction and `accent-soft` for tinted selection. `ok`, `warn`, and `bad` are status/destruction colors, paired with soft backgrounds. Primary and danger buttons use their dedicated ink tokens. Dark mode replaces the same CSS roles; component code does not invent a second palette.

**The Status Rule.** A status uses words and an icon, with a semantic tint as reinforcement. A color-only dot does not establish state.

Text/background pairs must meet WCAG AA contrast for their actual size. Verify tinted badges, danger labels, placeholders, and focus in both themes when changing colors; a compliant text token on surface does not establish contrast on every ground.

## Typography

The platform sans stack carries body copy, labels, buttons, and headings. Body is 14px at 1.55; page titles are 20px/600, section titles 15px/600, controls 13.5px/500, captions 12.5px. The connection title, large statistics, and device code may use their existing larger scale. Avoid display typography and all-caps voice.

**The Data Rule.** Machine identifiers, prefixes, counts, and timestamps use the mono stack and tabular figures; explanatory prose stays sans. Isolate machine text as LTR and foreign/user text with automatic direction.

## Layout

Group one subject in each card: state, settings, credentials, membership, permissions, or danger. Use the existing 8/12/16/20/24px rhythm; cards have 20px padding. Page headings identify the task and carry its primary action. Detail facts expand from one to two columns at the small breakpoint; narrow screens stack related controls without hiding operations. Long names and IDs wrap or scroll deliberately.

**The Ownership Rule.** Rows link to the resource that owns their state. Refresh does not erase a dirty draft, and a save result stays beside its action.

Keep physical inventory and automatic/authored public entries visibly distinguishable. Permission forms group typed owners and their routes; view filters do not mutate hidden choices. Translation can change label length, so use logical spacing and flexible layouts.

## Elevation & Depth

Surfaces are flat and separated by 1px rules. Cards have no shadow. Hover changes border or ground rather than lifting a surface. Focus uses a 2px accent outline with a 2px offset; keyboard focus must remain visible on every interactive element.

Motion is CSS only: 150ms ease-out interactive transitions, one 200ms rise-in per screen, a 1.4-second skeleton wash, and switch travel. No child choreography or animated table rows. Under `prefers-reduced-motion`, animations stop and transitions collapse to 0.01ms.

## Shapes

Cards use 16px radii, notices 12px, fields/buttons 8px, and badges/switches a pill radius. Inputs use a strong border and canvas ground, with muted ground in dark mode where implemented. Buttons share padding and typography across primary, secondary, danger, and ghost tones. Do not introduce a new shape to distinguish an ordinary domain entity.

## Components

Buttons name actions and busy states; disabled/busy controls cannot submit again. Fields have accessible labels, hints, and field-local error text. Switches are labelled enabled-state controls with on/off words; they do not submit the containing form. A card's heading and actions belong to its subject.

**The Notice Rule.** State the action result where it happened. Use an authored title, preserve the gateway detail, and name a recovery only when known. Failure notices use role="alert".

Badges combine icon and words on a soft ground. Destructive actions arm an inline confirmation with a clear consequence and cancel action. First-load skeletons hold structure, then yield to content or failure. Empty states explain the missing resource and the next action. Clickable figures and rows carry navigation affordances. Charts use accent bars, a simple axis, and keyboard-readable day/value information; data must not depend on hover.

## Do's and Don'ts

### Do

- Do reuse semantic tokens in both themes.
- Do use clear labels, explicit unknown values, and labelled status icons.
- Do group related settings and keep save/revert actions with their form.
- Do accommodate translated copy and isolate machine values.
- Do preserve keyboard focus and respect reduced motion.

### Don't

- Don't introduce inline row editors or ordinary confirmation modals.
- Don't use color as the only signal or conflate permission with availability.
- Don't add shadows, gradients, glass, display fonts, or external visual assets.
- Don't animate table rows or orchestrate page children.
- Don't encode English-only text or volatile page inventories in visual tokens.
