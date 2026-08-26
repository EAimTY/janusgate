import { expect, test } from "vitest";
import {
    CredentialsCipher,
    parseOpenAiCompatibleCredentials,
    parseOpenAiOAuthCredentials,
} from "/common/credentials";
import { ConfigurationError, CredentialDecryptionError, ValidationError } from "/common/errors";

const KEY_BASE64 = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";
const OTHER_KEY_BASE64 = "Hh8cHRobGBkWFxQVEhMSEQ4PDA0KCwgJBgUEAwIBAAA=";

test("credentials encrypt as IV followed by ciphertext and the GCM tag", async () => {
    const cipher = await CredentialsCipher.create(KEY_BASE64);
    const credentials = {
        accessToken: "access",
        accountId: "workspace",
        refreshToken: "refresh",
    };
    const first = await cipher.encrypt(credentials);
    const second = await cipher.encrypt(credentials);

    expect(first.byteLength).toBeGreaterThanOrEqual(12 + 16 + 1);
    expect(parseOpenAiOAuthCredentials(await cipher.decrypt(first))).toEqual(credentials);
    expect(first).not.toEqual(second);
});

test("a different AES key cannot decrypt stored credentials", async () => {
    const cipher = await CredentialsCipher.create(KEY_BASE64);
    const wrongCipher = await CredentialsCipher.create(OTHER_KEY_BASE64);
    const encrypted = await cipher.encrypt({
        apiKey: "secret",
    });

    await expect(wrongCipher.decrypt(encrypted)).rejects.toBeInstanceOf(CredentialDecryptionError);
});

test("the AES secret must decode to exactly 32 bytes", async () => {
    await expect(CredentialsCipher.create("c2hvcnQ=")).rejects.toBeInstanceOf(ConfigurationError);
    await expect(CredentialsCipher.create("not base64")).rejects.toBeInstanceOf(ConfigurationError);
});

test("credential payload parsing validates shape but not field contents", () => {
    expect(
        parseOpenAiCompatibleCredentials({
            apiKey: "",
        }),
    ).toEqual({
        apiKey: "",
    });
    expect(() =>
        parseOpenAiOAuthCredentials({
            accessToken: "a",
            refreshToken: "r",
        }),
    ).toThrow(ValidationError);
});
