import { describe, expect, it } from "vitest";
import { ManagementApiError } from "/src/management-api/errors";
import { ErrorCode } from "/src/management-api/generated/v0";
import { type ReadCall, ReadCallFailure, readAll, readOne } from "./read";

function gatewayFailure(errno: ErrorCode, message: string): ManagementApiError {
    return new ManagementApiError("gateway", message, {
        errno,
        httpStatus: 404,
    });
}

function abortFailure(): Promise<never> {
    return Promise.reject(new DOMException("The operation was aborted.", "AbortError"));
}

function call<TResult>(result: Promise<TResult>): ReadCall<TResult> {
    return {
        run: () => result,
    };
}

describe("readAll", () => {
    it("answers every call's result in call order", async () => {
        const controller = new AbortController();

        const [first, second] = await readAll(
            controller.signal,
            call(
                Promise.resolve({
                    providers: [],
                }),
            ),
            call(
                Promise.resolve({
                    models: [],
                }),
            ),
        );

        expect(first).toEqual({
            providers: [],
        });
        expect(second).toEqual({
            models: [],
        });
    });

    it("fails with the first failed call's diagnosis when several calls fail", async () => {
        const controller = new AbortController();

        const failure = await readAll(
            controller.signal,
            {
                run: () => Promise.reject(gatewayFailure(ErrorCode.NotFound, "gone row")),
            },
            {
                run: () => Promise.reject(gatewayFailure(ErrorCode.Conflict, "taken name")),
            },
        ).catch((error: unknown) => error);

        expect(failure).toBeInstanceOf(ReadCallFailure);
        expect((failure as ReadCallFailure).notice.tone).toBe("bad");
        expect((failure as ReadCallFailure).notice.detail).toBe("gone row");
    });

    it("treats an aborted call as the cycle's end, ahead of any failure", async () => {
        const controller = new AbortController();

        const failure = await readAll(controller.signal, call(abortFailure()), {
            run: () => Promise.reject(gatewayFailure(ErrorCode.NotFound, "gone row")),
        }).catch((error: unknown) => error);

        expect((failure as Error).name).toBe("AbortError");
    });
});

describe("readOne", () => {
    it("answers the call's result", async () => {
        const controller = new AbortController();

        const result = await readOne(
            controller.signal,
            call(
                Promise.resolve({
                    buckets: [],
                }),
            ),
        );

        expect(result).toEqual({
            buckets: [],
        });
    });

    it("fails with the call's diagnosis, carrying its action into the fix", async () => {
        const controller = new AbortController();

        const failure = await readOne(controller.signal, {
            action: "query-usage",
            run: () => Promise.reject(gatewayFailure(ErrorCode.UpstreamError, "upstream down")),
        }).catch((error: unknown) => error);

        expect(failure).toBeInstanceOf(ReadCallFailure);
        expect((failure as ReadCallFailure).notice.detail).toBe("upstream down");
        expect((failure as ReadCallFailure).notice.fix).toContain("credentials");
    });

    it("propagates an abort as control flow", async () => {
        const controller = new AbortController();

        const failure = await readOne(controller.signal, call(abortFailure())).catch(
            (error: unknown) => error,
        );

        expect((failure as Error).name).toBe("AbortError");
    });
});
