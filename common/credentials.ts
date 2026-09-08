import { decodeBase64 } from "/common/base64";
import type {
    AntigravityOAuthCredentials,
    OpenAiCompatibleCredentials,
    OpenAiOAuthCredentials,
    ProviderCredentials,
} from "/common/domain";
import { ConfigurationError, CredentialDecryptionError, ValidationError } from "/common/errors";
import { requireJsonObject, requireStringField } from "/common/validation";

const AES_256_KEY_BYTES = 32;
const AES_GCM_IV_BYTES = 12;
const AES_GCM_TAG_BYTES = 16;

/**
 * Decodes the `CREDENTIALS_AES_KEY` secret: strict standard base64 (canonical
 * padding, no whitespace, URL-safe alphabet rejected) that must yield exactly the
 * AES-256 key length.
 */
function decodeBase64Key(encodedKey: string): Uint8Array {
    let keyBytes: Uint8Array;
    try {
        keyBytes = decodeBase64(encodedKey);
    } catch {
        throw new ConfigurationError("CREDENTIALS_AES_KEY must be standard base64");
    }
    if (keyBytes.length !== AES_256_KEY_BYTES) {
        throw new ConfigurationError(
            `CREDENTIALS_AES_KEY must decode to exactly ${AES_256_KEY_BYTES} bytes`,
        );
    }
    return keyBytes;
}

export class CredentialsCipher {
    readonly #cryptography: Crypto;
    readonly #key: CryptoKey;

    private constructor(cryptography: Crypto, key: CryptoKey) {
        this.#cryptography = cryptography;
        this.#key = key;
    }

    public static async create(
        encodedKey: string,
        cryptography: Crypto = crypto,
    ): Promise<CredentialsCipher> {
        const keyBytes = decodeBase64Key(encodedKey);
        const key = await cryptography.subtle.importKey(
            "raw",
            keyBytes,
            {
                name: "AES-GCM",
            },
            false,
            [
                "decrypt",
                "encrypt",
            ],
        );
        return new CredentialsCipher(cryptography, key);
    }

    public async encrypt(credentials: ProviderCredentials): Promise<Uint8Array> {
        const initializationVector = this.#cryptography.getRandomValues(
            new Uint8Array(AES_GCM_IV_BYTES),
        );
        const plaintext = new TextEncoder().encode(JSON.stringify(credentials));
        const ciphertextAndTag = new Uint8Array(
            await this.#cryptography.subtle.encrypt(
                {
                    iv: initializationVector,
                    name: "AES-GCM",
                    tagLength: AES_GCM_TAG_BYTES * 8,
                },
                this.#key,
                plaintext,
            ),
        );

        const encrypted = new Uint8Array(initializationVector.length + ciphertextAndTag.length);
        encrypted.set(initializationVector);
        encrypted.set(ciphertextAndTag, initializationVector.length);
        return encrypted;
    }

    public async decrypt(encrypted: ArrayBuffer | Uint8Array): Promise<unknown> {
        const bytes = encrypted instanceof Uint8Array ? encrypted : new Uint8Array(encrypted);
        if (bytes.byteLength <= AES_GCM_IV_BYTES + AES_GCM_TAG_BYTES) {
            throw new CredentialDecryptionError();
        }

        const initializationVector = bytes.slice(0, AES_GCM_IV_BYTES);
        const ciphertextAndTag = bytes.slice(AES_GCM_IV_BYTES);
        try {
            const plaintext = await this.#cryptography.subtle.decrypt(
                {
                    iv: initializationVector,
                    name: "AES-GCM",
                    tagLength: AES_GCM_TAG_BYTES * 8,
                },
                this.#key,
                ciphertextAndTag,
            );
            const json = new TextDecoder("utf-8", {
                fatal: true,
                ignoreBOM: false,
            }).decode(plaintext);
            const value: unknown = JSON.parse(json);
            return value;
        } catch {
            throw new CredentialDecryptionError();
        }
    }
}

export function parseOpenAiOAuthCredentials(value: unknown): OpenAiOAuthCredentials {
    const object = requireJsonObject(value, "OpenAI credentials");
    return {
        accessToken: requireStringField(object, "accessToken"),
        accountId: requireStringField(object, "accountId"),
        refreshToken: requireStringField(object, "refreshToken"),
    };
}

function parseAntigravityOAuthCredentials(value: unknown): AntigravityOAuthCredentials {
    const object = requireJsonObject(value, "Antigravity credentials");
    return {
        accessToken: requireStringField(object, "accessToken"),
        projectId: requireStringField(object, "projectId"),
        refreshToken: requireStringField(object, "refreshToken"),
    };
}

function parseOpenAiCompatibleCredentials(value: unknown): OpenAiCompatibleCredentials {
    const object = requireJsonObject(value, "OpenAI-compatible credentials");
    return {
        apiKey: requireStringField(object, "apiKey"),
    };
}

export function assertCredentialsMatchProvider(
    providerType: "openai_oauth",
    credentials: unknown,
): OpenAiOAuthCredentials;
export function assertCredentialsMatchProvider(
    providerType: "antigravity_oauth",
    credentials: unknown,
): AntigravityOAuthCredentials;
export function assertCredentialsMatchProvider(
    providerType: "openai_compatible",
    credentials: unknown,
): OpenAiCompatibleCredentials;
export function assertCredentialsMatchProvider(
    providerType: "openai_oauth" | "antigravity_oauth" | "openai_compatible",
    credentials: unknown,
): ProviderCredentials {
    switch (providerType) {
        case "openai_oauth":
            return parseOpenAiOAuthCredentials(credentials);
        case "antigravity_oauth":
            return parseAntigravityOAuthCredentials(credentials);
        case "openai_compatible":
            return parseOpenAiCompatibleCredentials(credentials);
        default:
            throw new ValidationError("Unknown provider type");
    }
}
