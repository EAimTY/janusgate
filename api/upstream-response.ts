import { upstreamResponseTooLargeError, upstreamTimeoutError } from "/common/errors";
import {
    MAX_RESPONSE_BYTES,
    SAFE_UPSTREAM_RESPONSE_HEADERS,
    UPSTREAM_IDLE_TIMEOUT_SECONDS,
} from "./config";
import { concatenateBytes } from "./http";

export function copySafeUpstreamHeaders(source: Headers): Headers {
    const headers = new Headers();
    for (const name of SAFE_UPSTREAM_RESPONSE_HEADERS) {
        const value = source.get(name);
        if (value !== null) headers.set(name, value);
    }
    return headers;
}

/** No upstream bytes arrived for `UPSTREAM_IDLE_TIMEOUT_SECONDS` (§11.2 idle timeout). */
export class UpstreamIdleTimeoutError extends Error {
    public constructor() {
        super("upstream idle timeout");
        this.name = "UpstreamIdleTimeoutError";
    }
}

export async function readBeforeIdleTimeout(
    reader: ReadableStreamDefaultReader<Uint8Array>,
    abortController: AbortController,
): Promise<ReadableStreamReadResult<Uint8Array>> {
    let timeout: ReturnType<typeof setTimeout> | undefined;
    try {
        return await Promise.race([
            reader.read(),
            new Promise<never>((_resolve, reject) => {
                timeout = setTimeout(() => {
                    abortController.abort("upstream idle timeout");
                    reject(new UpstreamIdleTimeoutError());
                }, UPSTREAM_IDLE_TIMEOUT_SECONDS * 1_000);
            }),
        ]);
    } finally {
        if (timeout !== undefined) clearTimeout(timeout);
    }
}

export async function relayNativeStreamingResponse(
    upstreamResponse: Response,
    abortController: AbortController,
): Promise<Response> {
    if (upstreamResponse.body === null) {
        return new Response(null, {
            headers: copySafeUpstreamHeaders(upstreamResponse.headers),
            status: upstreamResponse.status,
        });
    }
    const reader = upstreamResponse.body.getReader();
    let firstRead: ReadableStreamReadResult<Uint8Array>;
    try {
        firstRead = await readBeforeIdleTimeout(reader, abortController);
    } catch {
        await reader.cancel("upstream idle timeout");
        throw upstreamTimeoutError();
    }

    let isCanceled = false;
    const body = new ReadableStream<Uint8Array>({
        async start(controller): Promise<void> {
            if (!firstRead.done) controller.enqueue(firstRead.value);
            else {
                controller.close();
                reader.releaseLock();
                return;
            }
            try {
                for (;;) {
                    const next = await readBeforeIdleTimeout(reader, abortController);
                    if (next.done) break;
                    controller.enqueue(next.value);
                }
            } catch {
                // Mid-stream failures are represented solely by an early EOF.
            } finally {
                if (!isCanceled) controller.close();
                reader.releaseLock();
            }
        },
        async cancel(reason): Promise<void> {
            isCanceled = true;
            abortController.abort(reason);
            await reader.cancel(reason);
        },
    });
    return new Response(body, {
        headers: copySafeUpstreamHeaders(upstreamResponse.headers),
        status: upstreamResponse.status,
    });
}

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
