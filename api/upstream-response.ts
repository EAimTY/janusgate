import { MILLISECONDS_PER_SECOND } from "/config";
import {
    MAX_RESPONSE_BYTES,
    SAFE_UPSTREAM_RESPONSE_HEADERS,
    UPSTREAM_IDLE_TIMEOUT_SECONDS,
} from "./config";
import {
    GatewayError,
    upstreamFailureError,
    upstreamResponseTooLargeError,
    upstreamTimeoutError,
} from "./errors";
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
        super("No upstream bytes arrived within the idle timeout");
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
                    // A single typed instance is both the abort reason seen by the upstream fetch
                    // and the rejection seen by the caller: the one signaling mechanism for the
                    // idle timeout (§11.2). There is deliberately no string sentinel.
                    const error = new UpstreamIdleTimeoutError();
                    abortController.abort(error);
                    reject(error);
                }, UPSTREAM_IDLE_TIMEOUT_SECONDS * MILLISECONDS_PER_SECOND);
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
    } catch (error) {
        // No reader.cancel here: the upstream stream is already dead (errored by the failure,
        // or aborted by the idle timeout or the client disconnect), and cancel() on an errored
        // stream rejects with the stored error, which would displace the classified envelope.
        // §11.2 first-byte classification: the typed idle timeout keeps the 504 envelope; every
        // other first-read failure (upstream died after sending headers) is an upstream condition
        // answered with the 502 envelope. A client-disconnect abort lands here too, but its
        // response has no receiver, so the uniform classification is unobservable.
        throw error instanceof UpstreamIdleTimeoutError
            ? upstreamTimeoutError()
            : upstreamFailureError();
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
    } catch (error) {
        // §11.2 classification for body reads: the typed idle timeout keeps the 504 envelope and
        // the size-limit GatewayError passes through untouched; any other read failure (upstream
        // disconnect, stream error) is an upstream condition — 502 upstream_error, never the
        // unclassified 500 the raw stream rejection used to produce.
        if (error instanceof UpstreamIdleTimeoutError) throw upstreamTimeoutError();
        if (error instanceof GatewayError) throw error;
        throw upstreamFailureError();
    } finally {
        reader.releaseLock();
    }
    return concatenateBytes(chunks, totalBytes);
}
