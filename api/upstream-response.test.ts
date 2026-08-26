import { describe, expect, it } from "vitest";
import { MAX_RESPONSE_BYTES } from "./config";
import { readIdleTimedUpstreamBody, relayNativeStreamingResponse } from "./upstream-response";

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
});
