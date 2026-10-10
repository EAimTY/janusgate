/**
 * The bucket-key contract of a `usage:query` answer (`docs/reference/management-api.md`): a
 * bucket key carries exactly the dimensions the request's `group_by` names.
 * Each aggregation dimension is paired with the wire field its key part rides
 * on, and a field the request's own dimension list guarantees is read as that
 * guaranteed value — an absent field is a contract violation, never a blank.
 */

import { ContractViolationError } from "/src/management-api/errors";
import type { UsageBucketKey, UsageGroupBy } from "/src/management-api/generated/v0";

/**
 * The one sentence a missing bucket-key field is stated with, whatever error
 * domain throws it: the read proves every requested dimension is present
 * while the cycle can still state its diagnosis, and the render asserts the
 * same invariant — two deliberate error domains over one shared sentence.
 */
export function missingBucketKeyFieldMessage(field: string): string {
    return `the bucket key carries no ${field}`;
}

/**
 * The bucket-key field each requested dimension's part rides on: the one
 * pairing the read's per-key proof (`readUsageBuckets`) and the render's
 * per-cell assert (`usageDimensionLabel`) both consume, so a dimension and
 * its wire field cannot drift apart. The literal-preserving type keeps every
 * entry's field exact, so a dimension narrowed by the dispatch reads its key
 * part with precisely the value type its renderer expects.
 */
export const DIMENSION_KEY_FIELD = {
    day: "day",
    model: "model_id",
    model_route: "model_route_id",
    provider: "provider_id",
    api_key: "api_key_id",
    endpoint_type: "endpoint_type",
    outcome: "outcome",
    is_stream: "is_stream",
    http_status: "http_status",
} as const satisfies Readonly<Record<UsageGroupBy, keyof UsageBucketKey>>;

/**
 * Reads one bucket key field the request's own dimension list guarantees:
 * a key carries exactly the requested dimensions, so an absent field is a
 * contract violation, not a blank to render past.
 */
export function bucketKeyField<TValue>(value: TValue | undefined, field: string): TValue {
    if (value === undefined) {
        throw new ContractViolationError(missingBucketKeyFieldMessage(field));
    }

    return value;
}
