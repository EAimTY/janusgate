import { ConfigurationError } from "/common/errors";

const SHA_256_BYTES = 32;
const SHA_256_HEX_CHARACTERS = SHA_256_BYTES * 2;
const SHA_256_HEX_PATTERN = /^[0-9a-f]{64}$/i;
const BEARER_PATTERN = /^Bearer ([^\s]+)$/i;

export interface AuthenticationSubtleCrypto {
    digest(algorithm: "SHA-256", data: BufferSource): Promise<ArrayBuffer>;
    timingSafeEqual(first: BufferSource, second: BufferSource): boolean;
}

function decodeSha256Hex(digestHex: string): Uint8Array {
    if (digestHex.length !== SHA_256_HEX_CHARACTERS || !SHA_256_HEX_PATTERN.test(digestHex)) {
        throw new ConfigurationError(
            "The configured token hash must be a 64-character SHA-256 hex digest",
        );
    }

    const digest = new Uint8Array(SHA_256_BYTES);
    for (let index = 0; index < digest.length; index += 1) {
        const pair = digestHex.slice(index * 2, index * 2 + 2);
        digest[index] = Number.parseInt(pair, 16);
    }
    return digest;
}

export async function verifyBearerAuthorization(
    authorizationHeader: string | null,
    expectedDigestHex: string,
    subtleCrypto: AuthenticationSubtleCrypto = crypto.subtle,
): Promise<boolean> {
    // Validate secrets even when the request is malformed: bad deployment configuration is fatal.
    const expectedDigest = decodeSha256Hex(expectedDigestHex);
    if (authorizationHeader === null) {
        return false;
    }

    const match = BEARER_PATTERN.exec(authorizationHeader);
    const token = match?.[1];
    if (token === undefined) {
        return false;
    }

    const presentedDigest = new Uint8Array(
        await subtleCrypto.digest("SHA-256", new TextEncoder().encode(token)),
    );
    return subtleCrypto.timingSafeEqual(presentedDigest, expectedDigest);
}
