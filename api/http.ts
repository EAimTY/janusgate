import { decodeUtf8Strict } from "/common/utf8";
import { isJsonObject } from "/common/validation";
import { type GatewayError, invalidJsonError, payloadTooLargeError } from "./errors";

/** Joins counted chunks into one exact-size buffer; `byteLength` must equal their sum. */
export function concatenateBytes(chunks: readonly Uint8Array[], byteLength: number): Uint8Array {
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
                return concatenateBytes(chunks, byteLength);
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

function parseJson(text: string): unknown {
    try {
        const value: unknown = JSON.parse(text);
        return value;
    } catch {
        throw invalidJsonError();
    }
}

function parseJsonObject(text: string): Record<string, unknown> {
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
        text = decodeUtf8Strict(bytes);
    } catch {
        throw invalidJsonError();
    }
    return parseJsonObject(text);
}
