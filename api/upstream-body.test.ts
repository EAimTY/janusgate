import { describe, expect, it } from "vitest";
import { MAX_RESPONSE_BYTES } from "./config";
import { readIdleTimedUpstreamBody } from "./upstream-body";

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
