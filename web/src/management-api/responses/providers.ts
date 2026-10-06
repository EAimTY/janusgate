import { ContractViolationError } from "../errors";
import type { ListProvidersResponse, ProviderEnvelope, ProviderResponse } from "../generated/v0";
import {
    expectBoolean,
    expectInteger,
    expectNullableInteger,
    expectNullableString,
    expectRecord,
    expectRowId,
    expectString,
    parseEnvelopeResponse,
    parseListResponse,
} from "../validation-primitives";
import { CREDENTIAL_KIND, UPSTREAM_FORMAT } from "../vocabulary";

/**
 * The definition pair, the derived format, and the nullable base URL: the
 * fields both credential kinds share, flattened into each union member.
 */
function parseSharedProviderFields(
    record: object,
    context: string,
): {
    readonly id: number;
    readonly name: string;
    readonly model_prefix: string;
    readonly is_enabled: boolean;
    readonly provider_definition_name: string;
    readonly provider_definition_version: number;
    readonly format: ProviderResponse["format"];
    readonly base_url: string | null;
} {
    return {
        id: expectRowId(Reflect.get(record, "id"), `${context}.id`),
        name: expectString(Reflect.get(record, "name"), `${context}.name`),
        model_prefix: expectString(Reflect.get(record, "model_prefix"), `${context}.model_prefix`),
        is_enabled: expectBoolean(Reflect.get(record, "is_enabled"), `${context}.is_enabled`),
        provider_definition_name: expectString(
            Reflect.get(record, "provider_definition_name"),
            `${context}.provider_definition_name`,
        ),
        provider_definition_version: expectInteger(
            Reflect.get(record, "provider_definition_version"),
            `${context}.provider_definition_version`,
        ),
        format: UPSTREAM_FORMAT.parse(Reflect.get(record, "format"), `${context}.format`),
        base_url: expectNullableString(Reflect.get(record, "base_url"), `${context}.base_url`),
    };
}

/**
 * One provider row; the union is discriminated by the row's credential kind.
 * Both members share the definition pair and the derived format; the OAuth
 * member alone carries the nullable `expires_at`. The kind membership reads
 * from the closed vocabulary, so a kind added to the contract cannot compile
 * past it and then fail here at runtime.
 */
export function parseProviderResponse(value: unknown, context: string): ProviderResponse {
    const record = expectRecord(value, context);
    const type = expectString(Reflect.get(record, "type"), `${context}.type`);
    const shared = parseSharedProviderFields(record, context);

    const kind = CREDENTIAL_KIND.find(type);

    if (kind === "oauth") {
        return {
            type: kind,
            ...shared,
            expires_at: expectNullableInteger(
                Reflect.get(record, "expires_at"),
                `${context}.expires_at`,
            ),
        };
    }

    if (kind === "api_key") {
        return {
            type: kind,
            ...shared,
        };
    }

    throw new ContractViolationError(`${context}.type is not a known credential kind: ${type}`);
}

export function parseListProvidersResponse(value: unknown): ListProvidersResponse {
    return parseListResponse(value, "providers", parseProviderResponse);
}

export function parseProviderEnvelope(value: unknown): ProviderEnvelope {
    return parseEnvelopeResponse(value, "provider", parseProviderResponse);
}
