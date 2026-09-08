/**
 * The single home for strict UTF-8 decoding over the Workers runtime
 * `TextDecoder` primitive. Decoding is fatal: any invalid byte sequence or
 * leading byte-order mark throws instead of decoding to replacement
 * characters, so callers can map the raw `TypeError` onto their own domain
 * error.
 */

const strictUtf8Decoder = new TextDecoder("utf-8", {
    fatal: true,
    ignoreBOM: false,
});

/**
 * Decodes bytes as UTF-8, rejecting invalid sequences and a leading
 * byte-order mark by throwing the decoder's raw `TypeError`. Because the
 * decode is non-streaming, the shared decoder retains no state between
 * calls.
 */
export function decodeUtf8Strict(bytes: BufferSource): string {
    return strictUtf8Decoder.decode(bytes);
}
