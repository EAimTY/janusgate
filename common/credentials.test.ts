import { expect, test } from "vitest";
import { assertCredentialsMatchProvider, CredentialsCipher } from "/common/credentials";
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
    expect(assertCredentialsMatchProvider("openai_oauth", await cipher.decrypt(first))).toEqual(
        credentials,
    );
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
    // Acceptance: each parser demands its own field set and never inspects contents.
    expect(
        assertCredentialsMatchProvider("openai_compatible", {
            apiKey: "",
        }),
    ).toEqual({
        apiKey: "",
    });
    expect(
        assertCredentialsMatchProvider("openai_oauth", {
            accessToken: "",
            accountId: "",
            refreshToken: "",
        }),
    ).toEqual({
        accessToken: "",
        accountId: "",
        refreshToken: "",
    });
    expect(
        assertCredentialsMatchProvider("antigravity_oauth", {
            accessToken: "",
            projectId: "",
            refreshToken: "",
        }),
    ).toEqual({
        accessToken: "",
        projectId: "",
        refreshToken: "",
    });
    // Rejection: payloads whose shape mismatches the provider type fail loudly.
    expect(() =>
        assertCredentialsMatchProvider("openai_oauth", {
            accessToken: "a",
            refreshToken: "r",
        }),
    ).toThrow(ValidationError);
    expect(() =>
        assertCredentialsMatchProvider("openai_compatible", {
            apiKey: 1,
        }),
    ).toThrow(ValidationError);
    expect(() =>
        assertCredentialsMatchProvider("antigravity_oauth", {
            accessToken: "a",
            refreshToken: "r",
        }),
    ).toThrow(ValidationError);
});
