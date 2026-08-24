import { type GatewayError, invalidJsonError, payloadTooLargeError } from "/common/errors";
import { isJsonObject } from "/common/validation";

function concatenate(chunks: readonly Uint8Array[], byteLength: number): Uint8Array {
    const bytes = new Uint8Array(byteLength);
    let offset = 0;
    for (const chunk of chunks) {
        bytes.set(chunk, offset);
        offset += chunk.byteLength;
    }
    return bytes;
}

async function cancelReader(reader: ReadableStreamDefaultReader<Uint8Array>): Promise<void> {
    try {
        await reader.cancel();
    } catch {
        // The size error is authoritative even if the source fails while being cancelled.
    }
}

async function readBoundedStream(
    stream: ReadableStream<Uint8Array> | null,
    maximumBytes: number,
    overflowError: () => GatewayError,
): Promise<Uint8Array> {
    if (!Number.isSafeInteger(maximumBytes) || maximumBytes < 0) {
        throw new RangeError("maximumBytes must be a non-negative safe integer");
    }
    if (stream === null) {
        return new Uint8Array();
    }

    const reader = stream.getReader();
    const chunks: Uint8Array[] = [];
    let byteLength = 0;
    try {
        while (true) {
            const result = await reader.read();
            if (result.done) {
                return concatenate(chunks, byteLength);
            }

            byteLength += result.value.byteLength;
            if (byteLength > maximumBytes) {
                await cancelReader(reader);
                throw overflowError();
            }
            chunks.push(result.value);
        }
    } finally {
        reader.releaseLock();
    }
}

function isDeclaredOversized(contentLength: string | null, maximumBytes: number): boolean {
    if (contentLength === null || !/^\d+$/.test(contentLength)) {
        return false;
    }
    const declaredBytes = Number(contentLength);
    return declaredBytes > maximumBytes;
}

export async function readBoundedRequestBody(
    request: Request,
    maximumBytes: number,
): Promise<Uint8Array> {
    if (isDeclaredOversized(request.headers.get("Content-Length"), maximumBytes)) {
        throw payloadTooLargeError();
    }
    return readBoundedStream(request.body, maximumBytes, payloadTooLargeError);
}

export function parseJson(text: string): unknown {
    try {
        const value: unknown = JSON.parse(text);
        return value;
    } catch {
        throw invalidJsonError();
    }
}

export function parseJsonObject(text: string): Record<string, unknown> {
    const value = parseJson(text);
    if (!isJsonObject(value)) {
        throw invalidJsonError();
    }
    return value;
}

export async function readBoundedJsonObject(
    request: Request,
    maximumBytes: number,
): Promise<Record<string, unknown>> {
    const bytes = await readBoundedRequestBody(request, maximumBytes);
    let text: string;
    try {
        text = new TextDecoder("utf-8", {
            fatal: true,
            ignoreBOM: false,
        }).decode(bytes);
    } catch {
        throw invalidJsonError();
    }
    return parseJsonObject(text);
}
