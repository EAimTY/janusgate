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
import { expectModelIdentifier } from "./identifiers";
import { parseQuotaRouting } from "./quota";
import {
    expectArray,
    expectBoolean,
    expectDeclaredRecord,
    expectNonEmptyString,
    expectOneOf,
    expectRecord,
    expectRowId,
    expectSafeInteger,
    expectString,
    parseListResponse,
} from "./validation-primitives";

/** One physical target, or one pool with its exact upstream identifier. */
function parseTarget(value: unknown, context: string): ModelRouteTarget {
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
            model_id: expectRowId(Reflect.get(record, "model_id"), `${context}.model_id`),
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
        pool_id: expectRowId(Reflect.get(record, "pool_id"), `${context}.pool_id`),
        upstream_id: expectModelIdentifier(
            Reflect.get(record, "upstream_id"),
            `${context}.upstream_id`,
        ),
    };
}

/** Complete authored membership retains missing or disabled accounts by identity. */
function parsePool(value: unknown, context: string): ProviderPoolResponse {
    const record = expectRecord(value, context);
    const providers = new Set<number>();
    const priorities = new Set<number>();
    const members = expectArray(Reflect.get(record, "members"), `${context}.members`).map(
        (value, index) => {
            const memberContext = `${context}.members[${index}]`;
            const member = expectRecord(value, memberContext);
            const provider_id = expectRowId(
                Reflect.get(member, "provider_id"),
                `${memberContext}.provider_id`,
            );
            const priority = expectSafeInteger(
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
        id: expectRowId(Reflect.get(record, "id"), `${context}.id`),
        name: expectString(Reflect.get(record, "name"), `${context}.name`),
        model_prefix: expectString(Reflect.get(record, "model_prefix"), `${context}.model_prefix`),
        is_enabled: expectBoolean(Reflect.get(record, "is_enabled"), `${context}.is_enabled`),
        members,
    };
}

/** Public route rows carry no synthesized physical provider or capability override. */
function parseRoute(value: unknown, context: string): ModelRouteResponse {
    const record = expectRecord(value, context);
    return {
        id: expectRowId(Reflect.get(record, "id"), `${context}.id`),
        public_id: expectModelIdentifier(Reflect.get(record, "public_id"), `${context}.public_id`),
        is_automatic: expectBoolean(Reflect.get(record, "is_automatic"), `${context}.is_automatic`),
        is_enabled: expectBoolean(Reflect.get(record, "is_enabled"), `${context}.is_enabled`),
        target: parseTarget(Reflect.get(record, "target"), `${context}.target`),
    };
}

/** Pools are a complete authored listing, with no upstream query. */
export function parseListProviderPoolsResponse(value: unknown): ListProviderPoolsResponse {
    return parseListResponse(value, "provider_pools", parsePool);
}

/** Public routes are decoded independently of upstream inventory. */
export function parseListModelRoutesResponse(value: unknown): ListModelRoutesResponse {
    return parseListResponse(value, "model_routes", parseRoute);
}

/** A created pool exposes only its permanent row identity. */
export function parseCreateProviderPoolResponse(value: unknown): CreateProviderPoolResponse {
    const record = expectRecord(value, "response");
    return {
        provider_pool_id: expectRowId(
            Reflect.get(record, "provider_pool_id"),
            "response.provider_pool_id",
        ),
    };
}

/** A created public route exposes only its permanent row identity. */
export function parseCreateModelRouteResponse(value: unknown): CreateModelRouteResponse {
    const record = expectRecord(value, "response");
    return {
        model_route_id: expectRowId(
            Reflect.get(record, "model_route_id"),
            "response.model_route_id",
        ),
    };
}

/** Refresh metadata contains safe categories, never a foreign observation or error body. */
function parseRefresh(value: unknown, context: string): LastRefresh {
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
    const attempted_at = expectSafeInteger(
        Reflect.get(record, "attempted_at"),
        `${context}.attempted_at`,
    );
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
            observed_at: expectSafeInteger(
                Reflect.get(record, "observed_at"),
                `${context}.observed_at`,
            ),
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
            const observed_at = expectSafeInteger(
                Reflect.get(record, "observed_at"),
                `${context}.observed_at`,
            );
            const rawReset = Reflect.get(record, "blocked_until");
            const blocked_until =
                rawReset === null ? null : expectSafeInteger(rawReset, `${context}.blocked_until`);
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
        last_refresh: parseRefresh(Reflect.get(record, "last_refresh"), "response.last_refresh"),
    };
}
