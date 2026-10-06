import { ContractViolationError } from "../errors";
import type {
    DefinitionEnvelope,
    DefinitionResponse,
    ListProviderDefinitionsResponse,
} from "../generated/v0";
import {
    expectDeclaredKeys,
    expectRecord,
    expectSafeInteger,
    expectString,
    isPresentValue,
    parseEnvelopeResponse,
    parseListResponse,
} from "../validation-primitives";
import { UPSTREAM_FORMAT } from "../vocabulary";
import { expectCompactionSection } from "./compaction";
import { expectCredential, expectProvisioning } from "./credentials";
import { expectDiscovery } from "./discovery";
import { expectInference } from "./inference";
import { parseQuotaDescriptor, validateQuotaRequest } from "./quota";
import type { DefinitionPayload, DefinitionPayloadResult } from "./types";

/**
 * Validates one authored definition payload (README §5): the JSON a preset
 * emitted or an operator edited. The gateway parses descriptors with
 * `deny_unknown_fields` and answers a broken payload with errno 1, so this
 * walk applies the same strictness before the round trip — every closed
 * vocabulary is checked against the generated types, every required key's
 * presence, and every undeclared key is named instead of silently dropped.
 */
export function validateDefinitionPayload(value: unknown): DefinitionPayloadResult {
    try {
        return {
            payload: expectDefinitionPayload(expectRecord(value, "definition"), "definition"),
        };
    } catch (error) {
        if (error instanceof ContractViolationError) {
            return {
                error: error.message,
            };
        }

        throw error;
    }
}

/**
 * One authored definition payload: the record narrowed, its declared keys
 * checked, then its sections rebuilt. The context is the JSON path the
 * messages name, `"definition"` at the top of an authored payload and the
 * response path inside a stored-definition parse.
 */
function expectDefinitionPayload(record: object, context: string): DefinitionPayload {
    expectDeclaredKeys(
        record,
        [
            "format",
            "inference",
            "credential",
            "provisioning",
            "discovery",
            "compaction",
            "quota",
        ],
        context,
    );

    return expectDefinitionSections(record, context);
}

/**
 * One definition payload's sections, rebuilt field by field from a record
 * whose declared keys the caller has already checked — the shared walk behind
 * the authored-payload gate and the stored-definition response parse. The
 * walk applies the gateway's provisioning-section rule too: the sequence runs
 * after the token exchange, so it carries at least one step when present and
 * only ever rides an OAuth credential.
 */
function expectDefinitionSections(record: object, context: string): DefinitionPayload {
    const format = UPSTREAM_FORMAT.parse(Reflect.get(record, "format"), `${context}.format`);
    const payload: DefinitionPayload = {
        format,
        inference: expectInference(
            Reflect.get(record, "inference"),
            `${context}.inference`,
            format,
        ),
        credential: expectCredential(Reflect.get(record, "credential"), `${context}.credential`),
    };

    const provisioning = Reflect.get(record, "provisioning");

    if (isPresentValue(provisioning)) {
        const steps = expectProvisioning(provisioning, `${context}.provisioning`);

        if (steps.length === 0) {
            throw new ContractViolationError(
                `${context}.provisioning must carry at least one step when present`,
            );
        }

        if (payload.credential.type !== "oauth") {
            throw new ContractViolationError(
                `${context}.provisioning requires an OAuth credential: it runs after the token exchange`,
            );
        }

        payload.provisioning = steps;
    }

    const discovery = Reflect.get(record, "discovery");

    if (isPresentValue(discovery)) {
        payload.discovery = expectDiscovery(discovery, `${context}.discovery`);
    }

    const compaction = Reflect.get(record, "compaction");

    if (isPresentValue(compaction)) {
        payload.compaction = expectCompactionSection(
            compaction,
            `${context}.compaction`,
            payload.format,
        );
    }

    const quota = Reflect.get(record, "quota");
    if (isPresentValue(quota)) {
        payload.quota = parseQuotaDescriptor(quota, `${context}.quota`);
        validateQuotaRequest(
            payload.quota,
            payload.credential,
            payload.provisioning,
            `${context}.quota.request`,
        );
    }
    return payload;
}

/**
 * One stored definition version: the `(name, version)` identity and the
 * creation timestamp — the fields the app reads — built beside the payload
 * the shared walk above reconstructs, so the value is assembled from
 * validated fields and never asserted. The row's own keys are declared first,
 * so an undeclared top-level member is named at its path instead of being
 * dropped by a pre-projected payload the way every nested section rejects.
 */
function parseDefinitionResponse(value: unknown, context: string): DefinitionResponse {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "name",
            "version",
            "created_at",
            "format",
            "inference",
            "credential",
            "provisioning",
            "discovery",
            "compaction",
            "quota",
        ],
        context,
    );

    const payload = expectDefinitionSections(record, context);

    return {
        name: expectString(Reflect.get(record, "name"), `${context}.name`),
        version: expectSafeInteger(Reflect.get(record, "version"), `${context}.version`, 1),
        created_at: expectSafeInteger(Reflect.get(record, "created_at"), `${context}.created_at`),
        ...payload,
    };
}

export function parseListProviderDefinitionsResponse(
    value: unknown,
): ListProviderDefinitionsResponse {
    return parseListResponse(value, "provider_definitions", parseDefinitionResponse);
}

export function parseDefinitionEnvelope(value: unknown): DefinitionEnvelope {
    return parseEnvelopeResponse(value, "provider_definition", parseDefinitionResponse);
}
