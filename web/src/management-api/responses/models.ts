import { ContractViolationError } from "../errors";
import type {
    DiscoverModelsResponse,
    ListModelsResponse,
    MechanismSupport,
    ModelEnvelope,
    ModelResponse,
} from "../generated/v0";
import {
    expectArray,
    expectBoolean,
    expectInteger,
    expectNullableInteger,
    expectRecord,
    expectString,
    parseEnvelopeResponse,
    parseListResponse,
} from "../validation-primitives";
import {
    COMPACTION_MECHANISM,
    DISCOVERY_STATE,
    MECHANISM_SUPPORT,
    REASONING_LEVEL,
} from "../vocabulary";

/**
 * One nullable string array: `null` stays `null` (the unknown state), an
 * array narrows each member to a string. Non-array, non-null input is a
 * contract violation.
 */
function expectNullableStringArray(value: unknown, context: string): Array<string> | null {
    if (value === null) {
        return null;
    }

    const entries = expectArray(value, context);

    return entries.map((entry, index) => expectString(entry, `${context}[${index}]`));
}

/**
 * One nullable reasoning-level set: `null` is unknown, an array narrows
 * each member against the closed vocabulary — a foreign level is a
 * contract violation, since both gateway write paths store canonicalized
 * vocabulary sets only.
 */
function expectNullableReasoningLevels(value: unknown, context: string): Array<string> | null {
    const levels = expectNullableStringArray(value, context);

    if (levels === null) {
        return null;
    }

    return levels.map((level, index) => REASONING_LEVEL.parse(level, `${context}[${index}]`));
}

/**
 * One nullable compaction-facts map: `null` is nothing known, an object
 * narrows each key against the closed mechanism vocabulary and each value
 * against the support states the column carries — the discovered base
 * stores known facts only, so `"unknown"` is foreign there, while the
 * administrator's override states it explicitly. A foreign mechanism or
 * support value is a contract violation, since both gateway write paths
 * store closed vocabulary members only.
 */
function expectNullableCompactionFacts<TSupport extends MechanismSupport>(
    value: unknown,
    context: string,
    supports: ReadonlyArray<TSupport>,
): {
    [mechanism: string]: TSupport;
} | null {
    if (value === null) {
        return null;
    }

    const record = expectRecord(value, context);
    const facts: {
        [mechanism: string]: TSupport;
    } = {};

    for (const key of Object.keys(record)) {
        const mechanismContext = `${context}.${key}`;
        const mechanism = COMPACTION_MECHANISM.find(key);

        if (mechanism === null) {
            throw new ContractViolationError(
                `${mechanismContext} is not a compaction mechanism: ${key}`,
            );
        }

        const support = supports.find(
            (candidate) => candidate === MECHANISM_SUPPORT.find(Reflect.get(record, key)),
        );

        if (support === undefined) {
            throw new ContractViolationError(
                `${mechanismContext} is not a mechanism support state: ${String(
                    Reflect.get(record, key),
                )}`,
            );
        }

        facts[mechanism] = support;
    }

    return facts;
}

/** One model row's discovered compaction facts: known facts only, never `"unknown"`. */
function expectDiscoveredCompactionFacts(
    value: unknown,
    context: string,
): ModelResponse["compaction_support"] {
    return expectNullableCompactionFacts(value, context, [
        "supported",
        "unsupported",
    ]);
}

/** One model row's compaction-facts override: the explicit `"unknown"` included. */
function expectCompactionOverrideFacts(
    value: unknown,
    context: string,
): ModelResponse["compaction_support_override"] {
    return expectNullableCompactionFacts(value, context, [
        "supported",
        "unsupported",
        "unknown",
    ]);
}

/**
 * One model row: its never-recycled row identifier, its routing string, its
 * own inputs — the nullable mirror column, the serving switch, the
 * discovery state — and its six capability columns: the three
 * discovery-owned base columns beside the three administrator-owned
 * override mirrors. The effective verdicts are never on the wire (rule 7).
 */
function parseModelResponse(value: unknown, context: string): ModelResponse {
    const record = expectRecord(value, context);

    return {
        id: expectInteger(Reflect.get(record, "id"), `${context}.id`),
        provider_id: expectInteger(Reflect.get(record, "provider_id"), `${context}.provider_id`),
        provider: expectString(Reflect.get(record, "provider"), `${context}.provider`),
        upstream_id: expectString(Reflect.get(record, "upstream_id"), `${context}.upstream_id`),
        is_enabled: expectBoolean(Reflect.get(record, "is_enabled"), `${context}.is_enabled`),
        discovery_state: DISCOVERY_STATE.parse(
            Reflect.get(record, "discovery_state"),
            `${context}.discovery_state`,
        ),
        thinking_levels: expectNullableReasoningLevels(
            Reflect.get(record, "thinking_levels"),
            `${context}.thinking_levels`,
        ),
        max_input_tokens: expectNullableInteger(
            Reflect.get(record, "max_input_tokens"),
            `${context}.max_input_tokens`,
        ),
        max_output_tokens: expectNullableInteger(
            Reflect.get(record, "max_output_tokens"),
            `${context}.max_output_tokens`,
        ),
        thinking_levels_override: expectNullableReasoningLevels(
            Reflect.get(record, "thinking_levels_override"),
            `${context}.thinking_levels_override`,
        ),
        max_input_tokens_override: expectNullableInteger(
            Reflect.get(record, "max_input_tokens_override"),
            `${context}.max_input_tokens_override`,
        ),
        max_output_tokens_override: expectNullableInteger(
            Reflect.get(record, "max_output_tokens_override"),
            `${context}.max_output_tokens_override`,
        ),
        compaction_support: expectDiscoveredCompactionFacts(
            Reflect.get(record, "compaction_support"),
            `${context}.compaction_support`,
        ),
        compaction_support_override: expectCompactionOverrideFacts(
            Reflect.get(record, "compaction_support_override"),
            `${context}.compaction_support_override`,
        ),
    };
}

export function parseListModelsResponse(value: unknown): ListModelsResponse {
    return parseListResponse(value, "models", parseModelResponse);
}

export function parseDiscoverModelsResponse(value: unknown): DiscoverModelsResponse {
    return parseListModelsResponse(value);
}

export function parseModelEnvelope(value: unknown): ModelEnvelope {
    return parseEnvelopeResponse(value, "model", parseModelResponse);
}
