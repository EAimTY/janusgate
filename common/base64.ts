/**
 * The single home for base64 and base64url codecs, implemented over the Workers
 * runtime primitives `atob`/`btoa` so that encoded output and decoding tolerance
 * match the platform exactly. Both decoders are strict: they accept only the exact
 * alphabet of their variant, reject every whitespace character, and enforce the
 * padding grammar documented per function — malformed input throws instead of
 * decoding to surprising bytes.
 */

/**
 * Decodes standard base64 (RFC 4648) into bytes.
 *
 * Accepts only the standard alphabet (`A-Z`, `a-z`, `0-9`, `+`, `/`) in canonical
 * form: `==` after two data characters, `=` after three, and no padding when the
 * length is a multiple of four. Whitespace is rejected everywhere, including the
 * whitespace `atob` alone would forgive. Non-zero trailing bits in the final data
 * character are tolerated (`atob` semantics inherited from the call sites this
 * module replaces). Throws on any other input.
 */
export function decodeBase64(text: string): Uint8Array {
    if (!/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(text)) {
        throw new Error("Text is not canonical standard base64");
    }
    return bytesFromBinary(atob(text));
}

/**
 * Decodes unpadded base64url (RFC 4648 §5 in the RFC 7515 JWS-segment form) into
 * bytes.
 *
 * Accepts only the URL-safe alphabet (`A-Z`, `a-z`, `0-9`, `-`, `_`) with at least
 * one character: `=` padding is rejected because JWT segments carry none and a
 * padded segment must fail loudly rather than silently decode; the standard `+`
 * and `/` characters are likewise rejected. Throws on any other input.
 */
export function decodeBase64Url(text: string): Uint8Array {
    if (!/^[A-Za-z0-9_-]+$/.test(text)) {
        throw new Error("Text is not unpadded base64url");
    }
    const standardBase64 = text.replaceAll("-", "+").replaceAll("_", "/");
    // Base64url omits padding; restore it because `atob` requires `=` whenever the
    // length is not a multiple of four.
    return bytesFromBinary(
        atob(standardBase64.padEnd(Math.ceil(standardBase64.length / 4) * 4, "=")),
    );
}

/** Encodes bytes as standard base64 (RFC 4648) with `=` padding. */
export function encodeBase64(bytes: Uint8Array): string {
    return btoa(binaryFromBytes(bytes));
}

/**
 * Encodes bytes as unpadded base64url (RFC 4648 §5): `+` and `/` become `-` and
 * `_`, and padding is omitted, keeping the output safe for URLs and OAuth
 * parameters.
 */
export function encodeBase64Url(bytes: Uint8Array): string {
    return encodeBase64(bytes).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}

/** Renders bytes as the one-character-per-byte binary string that `btoa` accepts. */
function binaryFromBytes(bytes: Uint8Array): string {
    let binary = "";
    for (const byte of bytes) binary += String.fromCharCode(byte);
    return binary;
}

/** Decodes the binary string `atob` returned into bytes. */
function bytesFromBinary(binary: string): Uint8Array {
    return Uint8Array.from(binary, (character) => character.charCodeAt(0));
}
