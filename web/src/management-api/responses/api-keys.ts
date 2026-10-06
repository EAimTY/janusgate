import { ContractViolationError } from "../errors";
import type {
    AccessDecision,
    ApiKeyAccessControl,
    ApiKeyEnvelope,
    ApiKeyResponse,
    ListApiKeysResponse,
} from "../generated/v0";
import {
    expectArray,
    expectBoolean,
    expectDeclaredRecord,
    expectOneOf,
    expectRecord,
    expectRowId,
    expectSafeInteger,
    expectString,
    parseEnvelopeResponse,
    parseListResponse,
} from "../validation-primitives";

/** One access decision: the closed allow/deny pair every policy member carries. */
function parseAccessDecision(value: unknown, context: string): AccessDecision {
    return expectOneOf(
        value,
        [
            "allow",
            "deny",
        ],
        context,
        "an access decision",
    );
}

function parseAccessControl(value: unknown, context: string): ApiKeyAccessControl {
    const record = expectDeclaredRecord(
        value,
        [
            "default_access",
            "owner_defaults",
            "route_overrides",
        ],
        context,
    );
    return {
        default_access: parseAccessDecision(
            Reflect.get(record, "default_access"),
            `${context}.default_access`,
        ),
        owner_defaults: expectArray(
            Reflect.get(record, "owner_defaults"),
            `${context}.owner_defaults`,
        ).map((value, index) => {
            const path = `${context}.owner_defaults[${index}]`;
            const owner = expectRecord(value, path);
            const type = Reflect.get(owner, "type");
            if (type === "provider") {
                expectDeclaredRecord(
                    owner,
                    [
                        "type",
                        "provider_id",
                        "access",
                    ],
                    path,
                );
                return {
                    type,
                    provider_id: expectRowId(
                        Reflect.get(owner, "provider_id"),
                        `${path}.provider_id`,
                    ),
                    access: parseAccessDecision(Reflect.get(owner, "access"), `${path}.access`),
                };
            }
            if (type === "pool") {
                expectDeclaredRecord(
                    owner,
                    [
                        "type",
                        "pool_id",
                        "access",
                    ],
                    path,
                );
                return {
                    type,
                    pool_id: expectRowId(Reflect.get(owner, "pool_id"), `${path}.pool_id`),
                    access: parseAccessDecision(Reflect.get(owner, "access"), `${path}.access`),
                };
            }
            throw new ContractViolationError(`${path}.type must be provider or pool`);
        }),
        route_overrides: expectArray(
            Reflect.get(record, "route_overrides"),
            `${context}.route_overrides`,
        ).map((value, index) => {
            const path = `${context}.route_overrides[${index}]`;
            const rule = expectDeclaredRecord(
                value,
                [
                    "model_route_id",
                    "access",
                ],
                path,
            );
            return {
                model_route_id: expectRowId(
                    Reflect.get(rule, "model_route_id"),
                    `${path}.model_route_id`,
                ),
                access: parseAccessDecision(Reflect.get(rule, "access"), `${path}.access`),
            };
        }),
    };
}

/** One gateway API key row: the complete readable state, plaintext secret included. */
function parseApiKeyResponse(value: unknown, context: string): ApiKeyResponse {
    const record = expectRecord(value, context);

    return {
        id: expectRowId(Reflect.get(record, "id"), `${context}.id`),
        name: expectString(Reflect.get(record, "name"), `${context}.name`),
        key: expectString(Reflect.get(record, "key"), `${context}.key`),
        is_enabled: expectBoolean(Reflect.get(record, "is_enabled"), `${context}.is_enabled`),
        created_at: expectSafeInteger(Reflect.get(record, "created_at"), `${context}.created_at`),
        access_control: parseAccessControl(
            Reflect.get(record, "access_control"),
            `${context}.access_control`,
        ),
    };
}

export function parseListApiKeysResponse(value: unknown): ListApiKeysResponse {
    return parseListResponse(value, "api_keys", parseApiKeyResponse);
}

export function parseApiKeyEnvelope(value: unknown): ApiKeyEnvelope {
    return parseEnvelopeResponse(value, "api_key", parseApiKeyResponse);
}
