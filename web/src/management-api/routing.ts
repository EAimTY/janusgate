import { expectOneOf } from "./definitions/validation";
import { ContractViolationError } from "./errors";
import type {
    CreateModelRouteResponse,
    CreateProviderPoolResponse,
    GetProviderAvailabilityResponse,
    LastRefresh,
    ListModelRoutesResponse,
    ListProviderPoolsResponse,
    ModelRouteResponse,
    ModelRouteTarget,
    ProviderPoolResponse,
    QuotaBlock,
} from "./generated/v0";
import { expectModelIdentifier, parseQuotaRouting } from "./quota";
import {
    expectArray,
    expectBoolean,
    expectDeclaredRecord,
    expectInteger,
    expectNonEmptyString,
    expectRecord,
    expectString,
    parseListResponse,
} from "./validation-primitives";

/** SQL row identifiers and priorities must survive JavaScript representation exactly. */
function integer(value: unknown, context: string, minimum?: number): number {
    const result = expectInteger(value, context);

    if (!Number.isSafeInteger(result) || (minimum !== undefined && result < minimum)) {
        throw new ContractViolationError(
            minimum === undefined
                ? `${context} must be a safe integer`
                : `${context} must be a safe integer of at least ${minimum}`,
        );
    }

    return result;
}

/** One physical target, or one pool with its exact upstream identifier. */
function target(value: unknown, context: string): ModelRouteTarget {
    const record = expectRecord(value, context);
    const type = expectOneOf(
        Reflect.get(record, "type"),
        [
            "direct",
            "pool",
        ],
        `${context}.type`,
        "a route target",
    );
    if (type === "direct") {
        expectDeclaredRecord(
            value,
            [
                "type",
                "model_id",
            ],
            context,
        );
        return {
            type,
            model_id: integer(Reflect.get(record, "model_id"), `${context}.model_id`, 1),
        };
    }
    expectDeclaredRecord(
        value,
        [
            "type",
            "pool_id",
            "upstream_id",
        ],
        context,
    );
    return {
        type,
        pool_id: integer(Reflect.get(record, "pool_id"), `${context}.pool_id`, 1),
        upstream_id: expectModelIdentifier(
            Reflect.get(record, "upstream_id"),
            `${context}.upstream_id`,
        ),
    };
}

/** Complete authored membership retains missing or disabled accounts by identity. */
function pool(value: unknown, context: string): ProviderPoolResponse {
    const record = expectRecord(value, context);
    const providers = new Set<number>();
    const priorities = new Set<number>();
    const members = expectArray(Reflect.get(record, "members"), `${context}.members`).map(
        (value, index) => {
            const memberContext = `${context}.members[${index}]`;
            const member = expectRecord(value, memberContext);
            const provider_id = integer(
                Reflect.get(member, "provider_id"),
                `${memberContext}.provider_id`,
                1,
            );
            const priority = integer(
                Reflect.get(member, "priority"),
                `${memberContext}.priority`,
                0,
            );
            if (providers.has(provider_id) || priorities.has(priority)) {
                throw new ContractViolationError(
                    `${memberContext} must not repeat an earlier provider_id or priority`,
                );
            }
            providers.add(provider_id);
            priorities.add(priority);
            return {
                provider_id,
                priority,
            };
        },
    );
    return {
        id: integer(Reflect.get(record, "id"), `${context}.id`, 1),
        name: expectString(Reflect.get(record, "name"), `${context}.name`),
        model_prefix: expectString(Reflect.get(record, "model_prefix"), `${context}.model_prefix`),
        is_enabled: expectBoolean(Reflect.get(record, "is_enabled"), `${context}.is_enabled`),
        members,
    };
}

/** Public route rows carry no synthesized physical provider or capability override. */
function route(value: unknown, context: string): ModelRouteResponse {
    const record = expectRecord(value, context);
    return {
        id: integer(Reflect.get(record, "id"), `${context}.id`, 1),
        public_id: expectModelIdentifier(Reflect.get(record, "public_id"), `${context}.public_id`),
        is_automatic: expectBoolean(Reflect.get(record, "is_automatic"), `${context}.is_automatic`),
        is_enabled: expectBoolean(Reflect.get(record, "is_enabled"), `${context}.is_enabled`),
        target: target(Reflect.get(record, "target"), `${context}.target`),
    };
}

/** Pools are a complete authored listing, with no upstream query. */
export function parseListProviderPoolsResponse(value: unknown): ListProviderPoolsResponse {
    return parseListResponse(value, "provider_pools", pool);
}

/** Public routes are decoded independently of upstream inventory. */
export function parseListModelRoutesResponse(value: unknown): ListModelRoutesResponse {
    return parseListResponse(value, "model_routes", route);
}

/** A created pool exposes only its permanent row identity. */
export function parseCreateProviderPoolResponse(value: unknown): CreateProviderPoolResponse {
    const record = expectRecord(value, "response");
    return {
        provider_pool_id: integer(
            Reflect.get(record, "provider_pool_id"),
            "response.provider_pool_id",
            1,
        ),
    };
}

/** A created public route exposes only its permanent row identity. */
export function parseCreateModelRouteResponse(value: unknown): CreateModelRouteResponse {
    const record = expectRecord(value, "response");
    return {
        model_route_id: integer(
            Reflect.get(record, "model_route_id"),
            "response.model_route_id",
            1,
        ),
    };
}

/** Refresh metadata contains safe categories, never a foreign observation or error body. */
function refresh(value: unknown, context: string): LastRefresh {
    const record = expectRecord(value, context);
    const type = expectOneOf(
        Reflect.get(record, "type"),
        [
            "not_observed",
            "unsupported",
            "reset",
            "succeeded",
            "failed",
        ],
        `${context}.type`,
        "a refresh outcome",
    );
    if (type === "not_observed") {
        expectDeclaredRecord(
            value,
            [
                "type",
            ],
            context,
        );
        return {
            type,
        };
    }
    const attempted_at = integer(Reflect.get(record, "attempted_at"), `${context}.attempted_at`);
    if (type === "unsupported" || type === "reset") {
        expectDeclaredRecord(
            value,
            [
                "type",
                "attempted_at",
            ],
            context,
        );
        return {
            type,
            attempted_at,
        };
    }
    if (type === "succeeded") {
        expectDeclaredRecord(
            value,
            [
                "type",
                "attempted_at",
                "observed_at",
            ],
            context,
        );
        return {
            type,
            attempted_at,
            observed_at: integer(Reflect.get(record, "observed_at"), `${context}.observed_at`),
        };
    }
    expectDeclaredRecord(
        value,
        [
            "type",
            "attempted_at",
            "reason",
        ],
        context,
    );
    return {
        type,
        attempted_at,
        reason: expectOneOf(
            Reflect.get(record, "reason"),
            [
                "credential_renewal",
                "timeout",
                "transport",
                "upstream_rejected",
                "malformed_observation",
            ],
            `${context}.reason`,
            "a refresh failure",
        ),
    };
}

/** Complete durable restrictions are interpreted without treating a due reset as recovery. */
export function parseGetProviderAvailabilityResponse(
    value: unknown,
): GetProviderAvailabilityResponse {
    const record = expectRecord(value, "response");
    const keys = new Set<string>();
    const blocks = expectArray(Reflect.get(record, "blocks"), "response.blocks").map(
        (value, index): QuotaBlock => {
            const context = `response.blocks[${index}]`;
            const record = expectRecord(value, context);
            const key = expectArray(Reflect.get(record, "key"), `${context}.key`).map(
                (value, index) => expectNonEmptyString(value, `${context}.key[${index}]`),
            );
            const identity = JSON.stringify(key);
            if (key.length === 0 || keys.has(identity)) {
                throw new ContractViolationError(
                    `${context}.key must not be empty or repeat another block's key`,
                );
            }
            keys.add(identity);
            const applies_to = parseQuotaRouting(
                Reflect.get(record, "applies_to"),
                `${context}.applies_to`,
            );
            if (applies_to.type === "monitoring_only") {
                throw new ContractViolationError(
                    `${context}.applies_to must not be monitoring_only`,
                );
            }
            const observed_at = integer(
                Reflect.get(record, "observed_at"),
                `${context}.observed_at`,
            );
            const rawReset = Reflect.get(record, "blocked_until");
            const blocked_until =
                rawReset === null ? null : integer(rawReset, `${context}.blocked_until`);
            if (blocked_until !== null && blocked_until <= observed_at) {
                throw new ContractViolationError(
                    `${context}.blocked_until must be after observed_at`,
                );
            }
            return {
                key,
                applies_to,
                observed_at,
                blocked_until,
            };
        },
    );
    return {
        blocks,
        last_refresh: refresh(Reflect.get(record, "last_refresh"), "response.last_refresh"),
    };
}
