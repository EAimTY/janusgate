import { expect, test } from "vitest";
import { verifyBearerAuthorization } from "/common/auth";
import { ConfigurationError } from "/common/errors";

const TOKEN = "gw_test-token";
const TOKEN_SHA256 = "b5e5d3a76f5564f996b162c37b0f88309bdc06c0110ce06d4db0ed9b3f1708b1";

function bufferSourceBytes(source: BufferSource): Uint8Array {
    if (source instanceof ArrayBuffer) {
        return new Uint8Array(source);
    }
    return new Uint8Array(source.buffer, source.byteOffset, source.byteLength);
}

test("bearer authentication hashes the presented token and uses timingSafeEqual", async () => {
    let timingSafeEqualCalls = 0;
    const subtleCrypto = {
        digest: crypto.subtle.digest.bind(crypto.subtle),
        timingSafeEqual(first: BufferSource, second: BufferSource): boolean {
            timingSafeEqualCalls += 1;
            const firstBytes = bufferSourceBytes(first);
            const secondBytes = bufferSourceBytes(second);
            return (
                firstBytes.length === secondBytes.length &&
                firstBytes.every((byte, index) => byte === secondBytes[index])
            );
        },
    };

    await expect(
        verifyBearerAuthorization(`Bearer ${TOKEN}`, TOKEN_SHA256, subtleCrypto),
    ).resolves.toBe(true);
    await expect(
        verifyBearerAuthorization("Bearer wrong", TOKEN_SHA256, subtleCrypto),
    ).resolves.toBe(false);
    expect(timingSafeEqualCalls).toBe(2);
});

test("malformed authorization headers never authenticate", async () => {
    const subtleCrypto = {
        digest: crypto.subtle.digest.bind(crypto.subtle),
        timingSafeEqual: () => true,
    };
    for (const header of [
        null,
        "",
        "Basic abc",
        "Bearer",
        "Bearer token extra",
    ]) {
        await expect(verifyBearerAuthorization(header, TOKEN_SHA256, subtleCrypto)).resolves.toBe(
            false,
        );
    }
});

test("a malformed configured digest is a fatal configuration error", async () => {
    await expect(verifyBearerAuthorization(`Bearer ${TOKEN}`, "abc")).rejects.toBeInstanceOf(
        ConfigurationError,
    );
});
