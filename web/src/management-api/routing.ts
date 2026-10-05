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
    RefreshFailure,
} from "./generated/v0";
import { parseQuotaRouting } from "./quota";
import {
    expectArray,
    expectBoolean,
    expectInteger,
    expectRecord,
    expectString,
    parseListResponse,
} from "./validation-primitives";

/** A management contract failure names only the safe structural path. */
function invalid(context: string): never {
    throw new ContractViolationError(`${context} violates the routing contract`);
}

/** Closed tagged values cannot hide a second target or foreign failure content. */
function closed(value: unknown, keys: readonly string[], context: string): object {
    const record = expectRecord(value, context);
    for (const key of Object.keys(record)) if (!keys.includes(key)) invalid(`${context}.${key}`);
    return record;
}

/** SQL row identifiers and priorities must survive JavaScript representation exactly. */
function integer(value: unknown, context: string, minimum?: number): number {
    const result = expectInteger(value, context);
    if (!Number.isSafeInteger(result) || (minimum !== undefined && result < minimum))
        invalid(context);
    return result;
}

/** Explicit public and physical model identifiers share the gateway's exact token contract. */
function token(value: unknown, context: string): string {
    const result = expectString(value, context);
    if (!/^[!-~]+$/.test(result) || /[/:]/.test(result)) invalid(context);
    return result;
}

/** One physical target, or one pool with its exact upstream identifier. */
function target(value: unknown, context: string): ModelRouteTarget {
    const record = expectRecord(value, context);
    const type = Reflect.get(record, "type");
    if (type === "direct") {
        closed(
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
    if (type === "pool") {
        closed(
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
            upstream_id: token(Reflect.get(record, "upstream_id"), `${context}.upstream_id`),
        };
    }
    return invalid(`${context}.type`);
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
            if (providers.has(provider_id) || priorities.has(priority)) invalid(memberContext);
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
        public_id: token(Reflect.get(record, "public_id"), `${context}.public_id`),
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
    const type = Reflect.get(record, "type");
    if (type === "not_observed") {
        closed(
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
        closed(
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
        closed(
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
    if (type === "failed") {
        closed(
            value,
            [
                "type",
                "attempted_at",
                "reason",
            ],
            context,
        );
        const reasons: readonly RefreshFailure[] = [
            "credential_renewal",
            "timeout",
            "transport",
            "upstream_rejected",
            "malformed_observation",
        ];
        const reason = reasons.find((reason) => reason === Reflect.get(record, "reason"));
        if (reason === undefined) invalid(`${context}.reason`);
        return {
            type,
            attempted_at,
            reason,
        };
    }
    return invalid(`${context}.type`);
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
                (value, index) => {
                    const segment = expectString(value, `${context}.key[${index}]`);
                    return segment.length > 0 ? segment : invalid(`${context}.key[${index}]`);
                },
            );
            const identity = JSON.stringify(key);
            if (key.length === 0 || keys.has(identity)) invalid(`${context}.key`);
            keys.add(identity);
            const applies_to = parseQuotaRouting(
                Reflect.get(record, "applies_to"),
                `${context}.applies_to`,
            );
            if (applies_to.type === "monitoring_only") invalid(`${context}.applies_to`);
            const observed_at = integer(
                Reflect.get(record, "observed_at"),
                `${context}.observed_at`,
            );
            const rawReset = Reflect.get(record, "blocked_until");
            const blocked_until =
                rawReset === null ? null : integer(rawReset, `${context}.blocked_until`);
            if (blocked_until !== null && blocked_until <= observed_at)
                invalid(`${context}.blocked_until`);
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
