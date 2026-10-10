import { isCredentialExpired, nowMicros } from "/src/format";
import type {
    MechanismSupport,
    ModelResponse,
    ModelRouteResponse,
    ProviderPoolResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import { type CompactionMechanism, isOAuthProviderRow } from "/src/management-api/vocabulary";

/**
 * The serving derivations the wire contract leaves to the client: rule 7
 * (`docs/reference/configuration.md` *Responses carry only what the caller cannot derive*) ships the
 * model row's inputs — never the three-input verdict over them — and the
 * provider row's credential columns — never the standing over them — so the
 * verdicts and the counts taken over the first are computed here, under the
 * gateway's own definitions (`docs/design/storage.md`). The generic row indexing these
 * derivations' callers share lives outside the contract, in `/src/row-index`.
 */

/**
 * Counts the listed models per provider row id, once per listings change,
 * through the one accumulation every inclusion shares. The optional predicate
 * states which rows count; without one, every row does. The providers page
 * states the raw row count on every row through this one map instead of
 * rebuilding the same counts.
 */
export function modelCountsByProvider(
    models: ReadonlyArray<ModelResponse>,
    isIncluded?: (model: ModelResponse) => boolean,
): ReadonlyMap<number, number> {
    const counts = new Map<number, number>();

    for (const model of models) {
        if (isIncluded === undefined || isIncluded(model)) {
            counts.set(model.provider_id, (counts.get(model.provider_id) ?? 0) + 1);
        }
    }

    return counts;
}

/** Counts ready physical inventory by account; readiness does not publish a public route. */
export function readyInventoryCountsByProvider(
    models: ReadonlyArray<ModelResponse>,
    providersById: ReadonlyMap<number, ProviderResponse>,
): ReadonlyMap<number, number> {
    return modelCountsByProvider(models, (model) =>
        isModelInventoryReady(model, providersById.get(model.provider_id)),
    );
}

/** Joins account/model switches and discovery state without claiming public-route or quota availability. */
export function isModelInventoryReady(
    model: ModelResponse,
    provider: ProviderResponse | undefined,
): boolean {
    return (
        (provider?.is_enabled ?? false) && model.is_enabled && model.discovery_state !== "vanished"
    );
}

/**
 * One model's effective thinking-level declaration (`docs/design/storage.md`):
 * the administrator's override when present, else the discovered set —
 * `null` when neither is known, the value every model listing omits. A
 * declaration, never an enforcement: dispatch never consults it.
 */
export function effectiveThinkingLevels(model: ModelResponse): ReadonlyArray<string> | null {
    return model.thinking_levels_override ?? model.thinking_levels;
}

/** One model's effective maximum-input-tokens declaration (`docs/design/storage.md`). */
export function effectiveMaxInputTokens(model: ModelResponse): number | null {
    return model.max_input_tokens_override ?? model.max_input_tokens;
}

/** One model's effective maximum-output-tokens declaration (`docs/design/storage.md`). */
export function effectiveMaxOutputTokens(model: ModelResponse): number | null {
    return model.max_output_tokens_override ?? model.max_output_tokens;
}

/**
 * Where one mechanism's effective compaction fact comes from: `"override"`
 * while the administrator's member stands for it, `"discovered"` while the
 * latest discovery knowledge stands, `"unknown"` while neither does.
 */
type CompactionFactSource = "override" | "discovered" | "unknown";

/** One mechanism's effective compaction fact beside the layer it came from. */
export type CompactionFact = {
    /**
     * The resolved support state: the override member, else the discovered
     * fact, else `"unknown"` — the state the gateway's own resolution lands
     * on when no row fact stands and the definition declares nothing.
     */
    readonly support: MechanismSupport;
    /** The layer the support was read from. */
    readonly source: CompactionFactSource;
};

/**
 * One mechanism's effective compaction fact: the administrator's override
 * member when one stands — the explicit `"unknown"` override included,
 * which suppresses the lower layers for that mechanism — else the
 * discovered fact when known, else `"unknown"`. The definition's
 * declaration, the resolution's third layer, is definition data the model
 * row never carries, so the row-level `"unknown"` never claims the
 * mechanism unsupported: at dispatch the definition's declaration decides.
 */
function resolveCompactionFact(
    model: ModelResponse,
    mechanism: CompactionMechanism,
): CompactionFact {
    const overridden = model.compaction_support_override?.[mechanism];

    if (overridden !== undefined) {
        return {
            support: overridden,
            source: "override",
        };
    }

    const discovered = model.compaction_support?.[mechanism];

    if (discovered !== undefined) {
        return {
            support: discovered,
            source: "discovered",
        };
    }

    return {
        support: "unknown",
        source: "unknown",
    };
}

/**
 * One model's effective compaction facts, per mechanism (`docs/design/storage.md`,
 * `docs/design/providers.md`). The record carries every mechanism, so a caller asks for the facts it
 * presents instead of re-deriving them; a mechanism added to the vocabulary
 * fails to compile here until the resolution carries it.
 */
export function effectiveCompactionFacts(
    model: ModelResponse,
): Readonly<Record<CompactionMechanism, CompactionFact>> {
    return {
        responses_standalone_compact: resolveCompactionFact(model, "responses_standalone_compact"),
        responses_explicit_trigger: resolveCompactionFact(model, "responses_explicit_trigger"),
        responses_automatic_compaction: resolveCompactionFact(
            model,
            "responses_automatic_compaction",
        ),
        messages_on_demand_compaction: resolveCompactionFact(
            model,
            "messages_on_demand_compaction",
        ),
        messages_threshold_compaction: resolveCompactionFact(
            model,
            "messages_threshold_compaction",
        ),
    };
}

/**
 * Where one effective capability declaration comes from: `"override"`
 * while the administrator's replacement stands, `"discovered"` while the
 * latest discovery knowledge stands, `"unknown"` while neither does —
 * the provenance the model page states beside each effective value.
 */
export type CapabilitySource = "override" | "discovered" | "unknown";

/**
 * The provenance of one effective declaration: the override's presence
 * decides, whatever the base holds — a standing override replaces the
 * declaration wholesale, even where the base is unknown.
 */
export function capabilitySource(
    override: ReadonlyArray<string> | number | null,
    base: ReadonlyArray<string> | number | null,
): CapabilitySource {
    if (override !== null) {
        return "override";
    }

    return base !== null ? "discovered" : "unknown";
}

/**
 * A provider row's credential standing — the one verdict every credential
 * presentation branches on: `"valid"` while a stored OAuth expiry still
 * holds, `"expired"` once it has passed, `"never-fresh"` when an OAuth row
 * stores no expiry at all, and `null` for a static-key row, which never
 * expires and so has no credential standing to state.
 */
export type CredentialStanding = "valid" | "expired" | "never-fresh" | null;

/**
 * Derives a provider row's credential standing from its own columns — the
 * single home of the verdict the providers table's credential badge, the
 * provider page's reconnect notice, and the overview's lapsed-credential
 * attention each branch on, so the three presentations cannot drift. An
 * OAuth row with no stored expiry was never fresh, an expiry already past
 * at the reading moment is expired, one still ahead is valid, and a static
 * key — which never expires — states no standing at all.
 */
export function credentialStanding(provider: ProviderResponse): CredentialStanding {
    if (!isOAuthProviderRow(provider)) {
        return null;
    }

    if (provider.expires_at === null) {
        return "never-fresh";
    }

    return isCredentialExpired(provider.expires_at, nowMicros()) ? "expired" : "valid";
}

/**
 * Whether a credential standing states a credential the gateway cannot use as
 * stored — the "needs a human" verdict over the standing: `"expired"` once
 * the stored OAuth expiry has passed, or `"never-fresh"` when an OAuth row
 * stores no expiry at all. A `"valid"` login is not lapsed, and a static
 * key — which never expires and states no standing at all — never is. The
 * internal disjunction the exported row-level verdict
 * `lapsedCredentialStanding` composes over `credentialStanding`'s
 * derivation.
 */
function isLapsedCredentialStanding(
    standing: CredentialStanding,
): standing is "expired" | "never-fresh" {
    return standing === "expired" || standing === "never-fresh";
}

/**
 * A provider row's credential standing when it names a credential the gateway
 * cannot use as stored — the "needs a human" verdict: `"expired"` once the
 * stored OAuth expiry has passed, or `"never-fresh"` when an OAuth row stores
 * no expiry at all — and `null` when there is nothing to show: a valid OAuth
 * login needs no human, and a static key, which never expires, states no
 * standing at all. The composition of `credentialStanding` with
 * `isLapsedCredentialStanding` over the row itself, hoisted beside its two
 * pieces so the overview's lapsed-credential attention, the provider page's
 * reconnect notice, and the edit page's reconnect offer state one verdict
 * instead of spelling the composition three times.
 */
export function lapsedCredentialStanding(
    provider: ProviderResponse,
): "expired" | "never-fresh" | null {
    const standing = credentialStanding(provider);

    return isLapsedCredentialStanding(standing) ? standing : null;
}

/** Automatic names belong to a standalone physical model or an exact model in its member pools. */
export function automaticModelNames(
    model: ModelResponse,
    routes: ReadonlyArray<ModelRouteResponse>,
    pools: ReadonlyArray<ProviderPoolResponse>,
): ReadonlyArray<string> {
    return routes
        .filter((route) => {
            if (!route.is_automatic) return false;
            const target = route.target;
            return target.type === "direct"
                ? target.model_id === model.id
                : target.upstream_id === model.upstream_id &&
                      pools.some(
                          (pool) =>
                              pool.id === target.pool_id &&
                              pool.members.some(
                                  (member) => member.provider_id === model.provider_id,
                              ),
                      );
        })
        .map((route) => route.public_id);
}
