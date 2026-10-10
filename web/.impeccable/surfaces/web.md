---
version: 1
slug: "web"
primary_target: "."
related_targets: []
---

# Surface brief: JanusGate Console

**Mode:** Operate. **Target:** the complete browser administration console.

**Audience and job.** One instance operator holding its management token, either connecting upstream accounts for the first time or returning to change one resource, recover credentials, inspect availability, manage permissions, or read traffic. Product intent comes from [PRODUCT.md](../../../PRODUCT.md); [DESIGN.md](../../DESIGN.md) owns the visual system.

**Action and proof.** Connect an account, obtain exact physical models and automatic public names, grant a serving key access, and hand its endpoint/model/key to a client. Show physical availability, route ownership, and permission provenance as separate facts. Report saves, discovery, reauthorization, quota reads/reset, and deletion where performed, retaining gateway detail.

**Constraints.** A static browser-only SPA talking directly to generated management API contracts. SolidJS, TypeScript, Vite, and Tailwind; system fonts and inline SVG, with no remote visual assets. Both themes, six supported languages, narrow layouts, keyboard interaction, visible focus, and reduced motion. Browser storage may fail, upstream evidence may be unknown, and refresh may race an active edit. The surface must preserve the documented form and connection lifetimes.

**Memorable moment.** The overview turns quiet numerical facts into doors, states what needs attention, and gives the operator the client endpoint paths. Setup proceeds directly from discovery to a permitted serving key; aliases are optional.

## Direction contract

THESIS: One page, one job. A resource has an obvious owner; its state is read separately from its edit, and results stay beside their action.

OWN-WORLD: Flat green-tinted neutral surfaces, teal actions, semantic icon-and-word statuses, fixed system typography, mono tabular machine values, bordered cards, and short CSS state transitions. Light and dark share semantic roles.

STORY: Connect the gateway, connect an upstream account, inventory models, inspect their automatic names, grant access, and configure a client. Return through the resource or attention item that owns the next job.

FIRST VIEWPORT: Shell navigation and connection context, then overview facts, traffic, attention, and endpoint handoff. The page identifies its current read/refresh state and a clear provider-creation action.

FORM: Settings-style navigation and dedicated resource pages. Pools and authored routes have their own editors; automatic entries point to their owner. Permission groups expose inheritance without changing hidden choices when filtered.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance
