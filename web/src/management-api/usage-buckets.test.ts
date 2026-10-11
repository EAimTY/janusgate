import { describe, expect, test } from "vitest";
import { bucketKeyField, DIMENSION_KEY_FIELD, missingBucketKeyFieldMessage } from "./usage-buckets";

/**
 * The bucket-key contract of a `usage:query` answer: the dimension↔field
 * pairing every consumer shares, the one missing-field sentence two error
 * domains state, and the fatal read of a field the request's own dimension
 * list guarantees.
 */

describe("DIMENSION_KEY_FIELD", () => {
    test("pairs every aggregation dimension with the wire field its key part rides on", () => {
        expect(DIMENSION_KEY_FIELD.day).toBe("day");
        expect(DIMENSION_KEY_FIELD.model).toBe("model_id");
        expect(DIMENSION_KEY_FIELD.model_route).toBe("model_route_id");
        expect(DIMENSION_KEY_FIELD.provider).toBe("provider_id");
        expect(DIMENSION_KEY_FIELD.api_key).toBe("api_key_id");
        expect(DIMENSION_KEY_FIELD.endpoint_type).toBe("endpoint_type");
        expect(DIMENSION_KEY_FIELD.outcome).toBe("outcome");
        expect(DIMENSION_KEY_FIELD.is_stream).toBe("is_stream");
        expect(DIMENSION_KEY_FIELD.http_status).toBe("http_status");
    });
});

describe("bucketKeyField", () => {
    test("answers a present value untouched — the NULL status bucket included", () => {
        expect(bucketKeyField(7, "model_id")).toBe(7);
        expect(bucketKeyField(null, "http_status")).toBeNull();
    });

    test("throws the shared missing-field sentence for an absent field", () => {
        expect(() => bucketKeyField(undefined, "day")).toThrowError(
            "the bucket key carries no day",
        );
    });
});

describe("missingBucketKeyFieldMessage", () => {
    test("states the one sentence the read and the render share", () => {
        expect(missingBucketKeyFieldMessage("outcome")).toBe("the bucket key carries no outcome");
    });
});
