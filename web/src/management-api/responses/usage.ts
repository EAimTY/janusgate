import type {
    ApiLogResponse,
    ListUsageResponse,
    QueryUsageResponse,
    UsageBucket,
    UsageBucketKey,
} from "../generated/v0";
import {
    expectBoolean,
    expectInteger,
    expectNullableInteger,
    expectRecord,
    expectRowId,
    expectSafeInteger,
    expectString,
    parseListResponse,
} from "../validation-primitives";
import { API_LOG_ENDPOINT_TYPE, API_LOG_OUTCOME } from "../vocabulary";

/**
 * One composite-key dimension: its wire field name and the narrowing that
 * types a present value. The narrowed type correlates with the name through
 * `UsageBucketKey`, so a dimension added to the contract extends the table
 * below, not the walk, and a mispaired narrowing cannot compile.
 */
type BucketKeyField = {
    readonly [TFieldName in keyof UsageBucketKey]-?: {
        readonly name: TFieldName;
        readonly narrow: (value: unknown, context: string) => UsageBucketKey[TFieldName];
    };
}[keyof UsageBucketKey];

/** Every dimension the composite bucket key can carry, in bindings order. */
const BUCKET_KEY_FIELDS: ReadonlyArray<BucketKeyField> = [
    {
        name: "day",
        narrow: expectString,
    },
    {
        name: "model_id",
        narrow: expectRowId,
    },
    {
        name: "model_route_id",
        narrow: expectRowId,
    },
    {
        name: "provider_id",
        narrow: expectRowId,
    },
    {
        name: "api_key_id",
        narrow: expectRowId,
    },
    {
        name: "endpoint_type",
        narrow: API_LOG_ENDPOINT_TYPE.parse,
    },
    {
        name: "outcome",
        narrow: API_LOG_OUTCOME.parse,
    },
    {
        name: "is_stream",
        narrow: expectBoolean,
    },
    {
        name: "http_status",
        narrow: expectNullableInteger,
    },
];

/**
 * Types one present dimension into the key through its correlated narrowing;
 * an absent one passes through, exactly as the wire omits it.
 */
function writeBucketKeyField<TFieldName extends keyof UsageBucketKey>(
    key: UsageBucketKey,
    field: {
        readonly name: TFieldName;
        readonly narrow: (value: unknown, context: string) => UsageBucketKey[TFieldName];
    },
    record: object,
    context: string,
): void {
    const value = Reflect.get(record, field.name);

    if (value !== undefined) {
        key[field.name] = field.narrow(value, `${context}.${field.name}`);
    }
}

function expectBucketKey(value: unknown, context: string): UsageBucketKey {
    const record = expectRecord(value, context);
    const key: UsageBucketKey = {};

    for (const field of BUCKET_KEY_FIELDS) {
        writeBucketKeyField(key, field, record, context);
    }

    return key;
}

/**
 * One accounting row, verbatim: the row's published accounting columns,
 * stamps and numeric facts only — no row id, no name rides along (rule 7).
 */
function parseApiLogResponse(value: unknown, context: string): ApiLogResponse {
    const record = expectRecord(value, context);

    return {
        started_at: expectSafeInteger(Reflect.get(record, "started_at"), `${context}.started_at`),
        api_key_id: expectRowId(Reflect.get(record, "api_key_id"), `${context}.api_key_id`),
        provider_id: expectRowId(Reflect.get(record, "provider_id"), `${context}.provider_id`),
        model_id: expectRowId(Reflect.get(record, "model_id"), `${context}.model_id`),
        model_route_id: expectRowId(
            Reflect.get(record, "model_route_id"),
            `${context}.model_route_id`,
        ),
        upstream_http_status: expectNullableInteger(
            Reflect.get(record, "upstream_http_status"),
            `${context}.upstream_http_status`,
        ),
        endpoint_type: API_LOG_ENDPOINT_TYPE.parse(
            Reflect.get(record, "endpoint_type"),
            `${context}.endpoint_type`,
        ),
        is_stream: expectBoolean(Reflect.get(record, "is_stream"), `${context}.is_stream`),
        http_status: expectNullableInteger(
            Reflect.get(record, "http_status"),
            `${context}.http_status`,
        ),
        outcome: API_LOG_OUTCOME.parse(Reflect.get(record, "outcome"), `${context}.outcome`),
        duration: expectInteger(Reflect.get(record, "duration"), `${context}.duration`),
        input_tokens: expectNullableInteger(
            Reflect.get(record, "input_tokens"),
            `${context}.input_tokens`,
        ),
        output_tokens: expectNullableInteger(
            Reflect.get(record, "output_tokens"),
            `${context}.output_tokens`,
        ),
        cached_tokens: expectNullableInteger(
            Reflect.get(record, "cached_tokens"),
            `${context}.cached_tokens`,
        ),
        cache_write_tokens: expectNullableInteger(
            Reflect.get(record, "cache_write_tokens"),
            `${context}.cache_write_tokens`,
        ),
        reasoning_tokens: expectNullableInteger(
            Reflect.get(record, "reasoning_tokens"),
            `${context}.reasoning_tokens`,
        ),
    };
}

/** One aggregation bucket: the grouping key and the log's own numeric sums. */
function parseUsageBucket(value: unknown, context: string): UsageBucket {
    const record = expectRecord(value, context);

    return {
        key: expectBucketKey(Reflect.get(record, "key"), `${context}.key`),
        requests: expectInteger(Reflect.get(record, "requests"), `${context}.requests`),
        input_tokens: expectInteger(Reflect.get(record, "input_tokens"), `${context}.input_tokens`),
        output_tokens: expectInteger(
            Reflect.get(record, "output_tokens"),
            `${context}.output_tokens`,
        ),
        cached_tokens: expectInteger(
            Reflect.get(record, "cached_tokens"),
            `${context}.cached_tokens`,
        ),
        cache_write_tokens: expectInteger(
            Reflect.get(record, "cache_write_tokens"),
            `${context}.cache_write_tokens`,
        ),
        reasoning_tokens: expectInteger(
            Reflect.get(record, "reasoning_tokens"),
            `${context}.reasoning_tokens`,
        ),
    };
}

export function parseListUsageResponse(value: unknown): ListUsageResponse {
    return parseListResponse(value, "api_logs", parseApiLogResponse);
}

export function parseQueryUsageResponse(value: unknown): QueryUsageResponse {
    return parseListResponse(value, "buckets", parseUsageBucket);
}
