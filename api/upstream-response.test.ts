import { describe, expect, it, vi } from "vitest";
import { MILLISECONDS_PER_SECOND } from "/config";
import { MAX_RESPONSE_BYTES, UPSTREAM_IDLE_TIMEOUT_SECONDS } from "./config";
import {
    readBeforeIdleTimeout,
    readIdleTimedUpstreamBody,
    relayNativeStreamingResponse,
    UpstreamIdleTimeoutError,
} from "./upstream-response";

describe("relayNativeStreamingResponse", () => {
    it("preserves status, safe headers, and body bytes", async () => {
        const bytes = new Uint8Array([
            0,
            1,
            2,
            255,
        ]);
        const upstream = new Response(bytes, {
            headers: {
                "Content-Type": "application/octet-stream",
                "Retry-After": "12",
                "Set-Cookie": "must-not-leak=1",
            },
            status: 429,
        });
        const result = await relayNativeStreamingResponse(upstream, new AbortController());

        expect(result.status).toBe(429);
        expect(result.headers.get("Content-Type")).toBe("application/octet-stream");
        expect(result.headers.get("Retry-After")).toBe("12");
        expect(result.headers.has("Set-Cookie")).toBe(false);
        expect(new Uint8Array(await result.arrayBuffer())).toEqual(bytes);
    });

    it("rejects with the 504 upstream_timeout envelope when the first byte never arrives", async () => {
        vi.useFakeTimers();
        try {
            const upstream = new Response(
                new ReadableStream<Uint8Array>({
                    start(): void {},
                }),
            );
            const abortController = new AbortController();
            const pending = relayNativeStreamingResponse(upstream, abortController);
            const expectation = expect(pending).rejects.toMatchObject({
                code: "upstream_timeout",
                status: 504,
            });
            await vi.advanceTimersByTimeAsync(
                UPSTREAM_IDLE_TIMEOUT_SECONDS * MILLISECONDS_PER_SECOND,
            );
            await expectation;
            expect(abortController.signal.reason).toBeInstanceOf(UpstreamIdleTimeoutError);
        } finally {
            vi.useRealTimers();
        }
    });

    it("rejects with the 502 upstream_failure envelope when the first read fails without a timeout", async () => {
        const upstream = new Response(
            new ReadableStream<Uint8Array>({
                start(controller): void {
                    controller.error(new Error("upstream connection reset"));
                },
            }),
        );
        await expect(
            relayNativeStreamingResponse(upstream, new AbortController()),
        ).rejects.toMatchObject({
            code: "upstream_failure",
            status: 502,
        });
    });
});

describe("readBeforeIdleTimeout", () => {
    it("aborts with the typed idle-timeout error as the signal reason and rejects with it", async () => {
        vi.useFakeTimers();
        try {
            const reader = new ReadableStream<Uint8Array>({
                start(): void {},
            }).getReader();
            const abortController = new AbortController();
            const pending = readBeforeIdleTimeout(reader, abortController);
            const expectation = expect(pending).rejects.toBeInstanceOf(UpstreamIdleTimeoutError);
            await vi.advanceTimersByTimeAsync(
                UPSTREAM_IDLE_TIMEOUT_SECONDS * MILLISECONDS_PER_SECOND,
            );
            await expectation;
            expect(abortController.signal.aborted).toBe(true);
            expect(abortController.signal.reason).toBeInstanceOf(UpstreamIdleTimeoutError);
        } finally {
            vi.useRealTimers();
        }
    });
});

describe("non-streaming upstream response limits", () => {
    it("aborts the upstream fetch as soon as the byte-counted limit is crossed", async () => {
        const chunkBytes = 1024 * 1024;
        const chunkCount = Math.floor(MAX_RESPONSE_BYTES / chunkBytes) + 1;
        const response = new Response(
            new ReadableStream<Uint8Array>({
                start(controller): void {
                    for (let index = 0; index < chunkCount; index += 1) {
                        controller.enqueue(new Uint8Array(chunkBytes));
                    }
                    controller.close();
                },
            }),
        );
        const abortController = new AbortController();

        await expect(readIdleTimedUpstreamBody(response, abortController)).rejects.toMatchObject({
            code: "upstream_response_too_large",
            status: 502,
        });
        expect(abortController.signal.aborted).toBe(true);
    });

    it("maps a non-timeout read failure to the 502 upstream_failure envelope", async () => {
        const response = new Response(
            new ReadableStream<Uint8Array>({
                start(controller): void {
                    controller.error(new Error("upstream connection reset"));
                },
            }),
        );
        await expect(
            readIdleTimedUpstreamBody(response, new AbortController()),
        ).rejects.toMatchObject({
            code: "upstream_failure",
            status: 502,
        });
    });
});
