# Console architecture

The console is a browser-only client of the [management API](../../docs/reference/management-api.md). It serves no gateway traffic and owns no durable gateway state. [PRODUCT.md](../../PRODUCT.md) owns intent; [DESIGN.md](../DESIGN.md) owns visual rules. The gateway's [design contracts](../../docs/README.md#design-and-contracts) apply to every editor and preview.

## Connection and boundary

The connection is a base URL plus raw management token. The browser calls the gateway directly with CORS, JSON POST bodies, and no cookies. A client is bound to one connection generation. Reconnection cancels outstanding requests; late responses cannot replace state or disconnect a newer client. A current client's 401 invalidates its token and returns to setup, retaining the address as a reconnect hint.

Only connection, theme, and language preferences are saved in localStorage. Storage failure is nonfatal and leaves in-memory state usable. OAuth flow state, editor drafts, fetched serving keys, and usage rows are not separately persisted. The stored management token is plaintext; there is no browser credential vault or server session.

The serde-aware [generated bindings](../../docs/development.md#workspace-and-commands) own methods, paths, payloads, and closed wire sets. Never hand-copy a management schema into frontend code. Network results are narrowed through explicit validation before entering UI state. Author complete write payloads by naming permitted fields, rather than spreading readable rows into requests. Human labels and machine identifiers use their distinct gateway boundary contracts.

## Navigation

The URL addresses the current job. Positive safe-integer row IDs identify resources; malformed IDs and unknown paths lead to not-found. Trailing slashes normalize to the same resource. List rows open a detail page, and edits have dedicated pages; pools and public routes use a shared editor for their detail/edit address family.

| Address | Job |
| --- | --- |
| `/` | Overview, attention, recent traffic, client endpoints |
| `/providers`, `/providers/new`, `/providers/:id`, `/providers/:id/edit` | Account lifecycle and credentials |
| `/models`, `/models/new`, `/models/:id`, `/models/:id/edit` | Physical inventory, capability facts, overrides |
| `/pools`, `/pools/new`, `/pools/:id`, `/pools/:id/edit` | Pool state and ordered membership |
| `/routes`, `/routes/new`, `/routes/:id`, `/routes/:id/edit` | Automatic entries and authored public aliases |
| `/routes/publish` | Bulk authored publication |
| `/api-keys`, `/api-keys/new`, `/api-keys/:id`, `/api-keys/:id/edit` | Serving secrets and model access |
| `/usage`, `/usage/query` | Attempt stream and aggregates |

Model lists/creation and bulk publication accept a `provider` filter; creation accepts an `upstream` claim prefill, and route creation accepts a `model` prefill. Usage listing carries its rolling-window/filter draft in query parameters; aggregates may preselect one entity subject and keep explicit time instants. Invalid filter inputs fall back to an unfiltered address rather than inventing an entity. OAuth flows remain steps inside provider forms, with no separate navigation destinations.

The shell sets a translated page title. Navigation and beforeunload consult the active form's dirty/OAuth guard; they do not maintain a second draft or dirty flag. A language or theme change does not recreate the client, page state, or active form.

## Reads, edits, and failures

A page reads on mount and explicit refresh. Page lifetime and read generation prevent stale completion from updating newer state. Reads never write after unmount. First-load skeletons become the read result or failure; stale rows may remain visible during refresh. Unknown-resource responses replace stale editors with not-found.

Edit forms hold the row from which their draft was created while dirty. Refresh may update surrounding read state, but cannot erase the draft or change its comparison baseline. A successful save adopts the returned/current state; revert explicitly discards the draft. Changed or deleted references remain visible, and invalid dangling permission references block save until removed. Search and permission-view filters do not change the policy draft.

Read failures and action notices are separate. A successful reread clears a read failure but cannot erase an action's result. Beginning another action or abandoning its form may clear that action notice. Errors retain the gateway's message as detail; recovery advice appears only when its cause is known. Destructive actions use an armed confirmation that names the affected resources.

## Domain presentation

Physical inventory, automatic public entries, authored aliases, and key permissions are separate facts. Discovery leads directly to callable names; alias creation is optional. Any pool membership suppresses standalone exposure even when disabled. Automatic routes are read-only and link to their owner. Prefix edits explain their rename effect; public-name collisions reject the complete write.

Permission previews use route override, typed owner default, then key default. Direct routes use provider ownership; pool routes use pool ownership. Global availability stays separate from effective key permission. Explicit equal-default pins remain editable pins. New keys start deny; editors send the complete atomic policy, including rules hidden by view filters.

Quota actions explicitly fetch or reset evidence. Ordinary page reads show stored availability and do not poll upstream. Declarations, administrator overrides, and effective capabilities are labelled separately; unknown is not false. Client previews use the same documented aggregation and resolution rules as the gateway.

Overview/usage displays aggregate numerical accounting, not request content. UTC accounting day buckets remain UTC; timestamps are rendered in the user's local timezone. Deleted entity IDs remain meaningful attribution even without current labels. Unknown measurements are displayed as unknown, not inferred zeros.
