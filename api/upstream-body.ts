import { upstreamResponseTooLargeError } from "/common/errors";
import { concatenateBytes } from "/common/http";
import { MAX_RESPONSE_BYTES } from "./config";
import { readBeforeIdleTimeout } from "./upstream-response";

export async function readIdleTimedUpstreamBody(
    response: Response,
    abortController: AbortController,
): Promise<Uint8Array> {
    if (response.body === null) return new Uint8Array();
    const reader = response.body.getReader();
    const chunks: Uint8Array[] = [];
    let totalBytes = 0;
    try {
        for (;;) {
            const result = await readBeforeIdleTimeout(reader, abortController);
            if (result.done) break;
            totalBytes += result.value.byteLength;
            if (totalBytes > MAX_RESPONSE_BYTES) {
                abortController.abort("upstream response size limit exceeded");
                await reader.cancel("upstream response size limit exceeded");
                throw upstreamResponseTooLargeError();
            }
            chunks.push(result.value);
        }
    } finally {
        reader.releaseLock();
    }
    return concatenateBytes(chunks, totalBytes);
}
