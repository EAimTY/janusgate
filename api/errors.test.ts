import { expect, test } from "vitest";
import { GatewayError, gatewayErrorBody, internalGatewayError } from "./errors";

test("gateway errors use the one normative OpenAI-shaped envelope", () => {
    expect(
        gatewayErrorBody(
            new GatewayError(
                404,
                "not_found_error",
                "The requested model was not found",
                "model_not_found",
            ),
        ),
    ).toEqual({
        error: {
            code: "model_not_found",
            message: "The requested model was not found",
            type: "not_found_error",
        },
    });
});

test("optional gateway error codes are omitted rather than serialized as undefined", () => {
    expect(gatewayErrorBody(new GatewayError(429, "rate_limit_error", "Slow down"))).toEqual({
        error: {
            message: "Slow down",
            type: "rate_limit_error",
        },
    });
});

test("internal errors do not expose thrown values", () => {
    expect(gatewayErrorBody(internalGatewayError())).toEqual({
        error: {
            code: "internal_error",
            message: "An internal error occurred",
            type: "api_error",
        },
    });
});
