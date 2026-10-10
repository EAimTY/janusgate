# Routing and access

A client selects a public model ID. A public route resolves that name to either one physical model or a pool plus an exact upstream ID. The upstream receives the physical ID unchanged; outward responses retain the requested public name.

## Inventory and automatic exposure

Physical models are unique by `(provider_id, upstream_id)` and addressed by stable row ID. Their discovery state is `discovered`, `vanished`, or `manual`. Discovery atomically marks missing discovered rows vanished and upserts reported rows; it never deletes rows or changes administrator switches/overrides. Manual rows do not become vanished merely because discovery omits them. Failed, empty, malformed, or incompletely paginated discovery leaves the inventory unchanged.

A vanished row is restored by rediscovery or claimed through `models:create`, retaining its identity. Creating an already discovered or manual natural key conflicts. Deleting a physical model removes its direct routes; a later discovery creates a new physical identity and new automatic exposure, without restoring deleted aliases or permission pins.

Physical readiness is `provider.is_enabled && model.is_enabled && discovery_state != "vanished"`. Readiness, public exposure, serving-key permission, and temporary quota availability are separate facts.

### Names and ownership

A provider outside every pool exposes each stored model as `provider.model_prefix + upstream_id`. Any pool membership suppresses standalone exposure, even membership in a disabled pool. Removing its last membership restores standalone entries.

Each pool exposes the union of member upstream IDs under `pool.model_prefix + upstream_id`. Disabled and vanished inventory keeps the automatic entry for recovery; removing its last inventory owner removes it. Prefixes may be empty. IDs are nonempty printable ASCII without whitespace, `/`, or `:` so they remain addressable through Gemini URL paths.

Automatic entries follow their provider/model or pool gates and cannot be edited or deleted as routes. Explicit aliases have their own public ID, switch, and direct/pool target. Retargeting and renaming preserve route identity. Prefix changes preserve identities of surviving automatic targets. Automatic synchronization and collision checks belong to the same mutation transaction; a naming conflict rolls back the whole mutation.

Deleting a provider cascades its physical models, direct routes, memberships, and quota restrictions. Deleting a pool conflicts while an authored route references it; otherwise it removes automatic pool entries and restores standalone exposure where appropriate.

## Serving-key permissions

Every serving key has a required `allow`/`deny` default, sparse typed provider/pool defaults, and sparse route overrides. The first present decision wins:

```text
route override → owning provider or pool default → key default
```

A direct route's owner is its physical provider. A pooled route's owner is the pool, never a member provider. An inherited deny can be overridden by a route allow. Inherit is absence of a rule; explicit decisions equal to inherited defaults remain authored pins. Defaults apply to existing and future inheriting routes.

Rules address never-recycled route and owner identities. Rename/retarget preserves route pins; retarget changes its inherited owner. Deletion removes rules for deleted identities, and recreation by name transfers no grants. Disabled or vanished targets keep surviving rules. Key create/update replaces ordinary fields and the complete policy atomically; duplicate references are 400, unknown references 404, and conflicts leave all state unchanged.

Inference, compaction, replay, and model listings use an enabled key and current route policy in one snapshot. Denied or absent routes return native model-not-found 404 before affinity, quota, renewal, or dispatch. A key deleted or disabled since authentication returns 401. Grants never override global routing gates. Already admitted work retains admission across bounded fallback and same-route compaction stages.

## Ordered pool selection

Pool membership is unique by provider and priority within the pool; priorities are nonnegative and need not be contiguous. Empty pools are valid. An enabled pooled route scans members by ascending priority, considering only enabled accounts with an enabled, non-vanished instance of the exact requested model. Inventory is never synthesized from another member.

| Condition | Result |
| --- | --- |
| Missing/disabled route, disabled pool, unavailable direct target | Model not found, 404 |
| Enabled pool route with no structural candidate | `pool_unavailable`, 503 |
| Every structural candidate blocked or positively rejecting quota | `pool_quota_exhausted`, 429 |
| Bound account absent or structurally unable to serve | `bound_account_unavailable`, 503 |
| Bound account blocked or positively rejecting quota | `bound_account_quota_exhausted`, 429 |
| Conflicting native/affinity requirements | `routing_state_conflict`, 400 |

Direct routes bypass affinity and quota preflight and dispatch once, even with recorded restrictions. Fresh pooled requests check candidate quota before dispatch. Known applicable blocks skip an account without an accounting row. Failed or unsupported optional quota reads preserve established blocks and otherwise leave the account eligible.

A fresh unbound request may advance only when the first unaccepted data-plane dispatch has a complete rejection body positively matching that definition's exact quota rule. Each account is tried once; an abandoned attempt finalizes before the next begins. A bare 429, timeout, overload, credential failure, malformed rejection, or ambiguous transport outcome does not permit switching. Successful 2xx headers commit the account before body parsing. A counter, accepted summary, or earlier native stage prevents later switching.

Exhaustion sends `Retry-After` only with complete future absolute-reset evidence. For each candidate, use its latest applicable reset; choose the earliest candidate with all restrictions accounted for and round positive seconds upward. Bound exhaustion uses only the required account. A passed reset means recheck, never proven replenishment.

## Quota evidence

Restrictions are account-owned and shared across pools, with explicit account-wide or exact-model applicability. Labels and presentation scopes never imply applicability. Observation rules default to monitoring-only. A restriction key is an ordered tuple, not a joined string.

Only positive exhaustion evidence creates a block: used ratio at least one, used counter at least its limit, remaining zero, or an exact classified inference rejection. Positive available or unlimited evidence clears its exact key. Missing, unknown, failed, and monitoring-only observations preserve blocks. Unknown reset times remain blocked until positive evidence or explicit reset.

Quota reads have one account-exclusive flight and live refresh cadence, including failed and unsupported reads. Forced Refresh joins current-generation work or waits for older work to finish. Automatic cadence can shorten to a future applicable reset; caller cancellation does not cancel the reading. Full observations apply only while both captured account generation and evidence revision match; dispatch rejection advances revision and defeats older reads.

Availability Reset clears restrictions and installs one automatic refresh interval of grace without resetting upstream quota. Explicit credential replacement, OAuth reauthorization, definition rebind, and base-URL change clear obsolete evidence transactionally. Rename, serving switches, and routine renewal preserve it. Listings and availability reads never poll quota. [Provider recipes](providers.md#quota) define extraction and rejection matching.

## Conversation affinity

A bounded in-process cache holds keyed fingerprints, account identities, and ownership/lifetime metadata in the `(api_key_id, pool_id)` namespace. It retains no conversation content and adds no database tables or client session IDs. Completed history prefers the longest known prefix; exact identities outrank relational tool identities at equal length. Ambiguous fingerprints cannot establish an account.

Changing sampling, instructions, stream mode, alias, or the next user turn does not move an established historical prefix. Switching to another model within the same pool still requires that account; failure is explicit even if another member can serve it. Concurrent matching requests share the account-establishment decision but execute independent generations after acceptance. Cancelling a follower does not cancel the leader; cancelling an unaccepted leader releases its reservation.

Hints derive only from actually emitted, replayable fields and completed artifacts. A complete checkpoint may establish affinity even when a later answer fails; incomplete state and undisclosed output cannot. Native carrier provenance binds its producer even after cache loss. Conflicting cached/native requirements fail before availability checks. Direct routes bypass the cache.

History/retry idle lifetimes and capacity are live configuration. Eviction, expiry, and restart lose ordinary hints; affinity is not durable session storage. Pending decisions are pinned, and capacity pressure may leave a request uncached. Deleting a pool or serving key invalidates its namespace and outstanding registration handles.

## Public model listings

Listings apply key policy and structural availability, order by public ID, and return the complete set in one page. Temporary quota restrictions do not hide models. Direct declarations resolve `override ?? discovered`; pooled numerical limits use minima and reasoning levels use intersection only when every structural candidate knows that fact. A known empty intersection stays empty. Pooled compaction is supported only when all candidates support it, unsupported when any rejects it, otherwise unknown. Unknown declarations are omitted, never defaulted. Declarations do not enforce ordinary inference support.
