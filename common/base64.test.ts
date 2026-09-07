import { describe, expect, test } from "vitest";
import { decodeBase64, decodeBase64Url, encodeBase64, encodeBase64Url } from "/common/base64";

function textBytes(text: string): Uint8Array {
    return new TextEncoder().encode(text);
}

/** RFC 4648 §10 test vectors, padded standard base64. */
const RFC_4648_VECTORS: readonly (readonly [
    string,
    string,
])[] = [
    [
        "",
        "",
    ],
    [
        "f",
        "Zg==",
    ],
    [
        "fo",
        "Zm8=",
    ],
    [
        "foo",
        "Zm9v",
    ],
    [
        "foob",
        "Zm9vYg==",
    ],
    [
        "fooba",
        "Zm9vYmE=",
    ],
    [
        "foobar",
        "Zm9vYmFy",
    ],
];

describe("encodeBase64", () => {
    test.each(RFC_4648_VECTORS)("encodes %#", (text, expected) => {
        expect(encodeBase64(textBytes(text))).toBe(expected);
    });

    test("encodes the URL-safe byte pair with the standard alphabet", () => {
        expect(
            encodeBase64(
                new Uint8Array([
                    0xfb,
                    0xff,
                ]),
            ),
        ).toBe("+/8=");
        expect(
            encodeBase64(
                new Uint8Array([
                    0xff,
                ]),
            ),
        ).toBe("/w==");
    });

    test("encodes Gemini inlineData payload shapes", () => {
        expect(encodeBase64(textBytes("<svg>"))).toBe("PHN2Zz4=");
        expect(
            encodeBase64(
                new Uint8Array([
                    0x00,
                ]),
            ),
        ).toBe("AA==");
    });
});

describe("decodeBase64", () => {
    test.each(RFC_4648_VECTORS)("decodes %#", (text, encoded) => {
        expect(decodeBase64(encoded)).toEqual(textBytes(text));
    });

    test("accepts canonical input whose length is a multiple of four without padding", () => {
        expect(decodeBase64("Zm9v")).toEqual(textBytes("foo"));
    });

    test("tolerates non-zero trailing bits in the final data character", () => {
        // "AB==" carries two non-zero discarded bits; `atob` decodes it to 0x00 and
        // the strict padding grammar still holds.
        expect(decodeBase64("AB==")).toEqual(
            new Uint8Array([
                0x00,
            ]),
        );
    });

    test("accepts the empty string as the empty byte sequence", () => {
        expect(decodeBase64("")).toEqual(new Uint8Array(0));
    });

    test("rejects missing padding", () => {
        expect(() => decodeBase64("Zg")).toThrow();
        expect(() => decodeBase64("Zm8")).toThrow();
        expect(() => decodeBase64("Zm9vYg=")).toThrow();
    });

    test("rejects misplaced or excess padding", () => {
        expect(() => decodeBase64("=Zm9v")).toThrow();
        expect(() => decodeBase64("Zm9v=")).toThrow();
        expect(() => decodeBase64("Zg===")).toThrow();
        expect(() => decodeBase64("====")).toThrow();
    });

    test("rejects whitespace, including the whitespace atob would forgive", () => {
        expect(() => decodeBase64(" Zm9vYg==")).toThrow();
        expect(() => decodeBase64("Zm9vYg== ")).toThrow();
        expect(() => decodeBase64("Zm9v Yg==")).toThrow();
        expect(() => decodeBase64("Zm9v\tYg==")).toThrow();
        expect(() => decodeBase64("Zm9vYg==\n")).toThrow();
    });

    test("rejects the URL-safe alphabet and other invalid characters", () => {
        expect(() => decodeBase64("-_8")).toThrow();
        expect(() => decodeBase64("Zm9v*")).toThrow();
    });
});

describe("encodeBase64Url", () => {
    test.each(RFC_4648_VECTORS)("encodes %# without padding", (text, padded) => {
        expect(encodeBase64Url(textBytes(text))).toBe(padded.replaceAll("=", ""));
    });

    test("maps +/ to -_ and keeps the output URL-safe", () => {
        expect(
            encodeBase64Url(
                new Uint8Array([
                    0xfb,
                    0xff,
                ]),
            ),
        ).toBe("-_8");
        expect(
            encodeBase64Url(
                new Uint8Array([
                    0xff,
                ]),
            ),
        ).toBe("_w");
    });
});

describe("decodeBase64Url", () => {
    // The empty vector is excluded: the JWS-segment grammar requires at least one
    // character, and the empty string is covered by the rejection test below.
    test.each(RFC_4648_VECTORS.filter(([text]) => text !== ""))(
        "decodes %# without padding",
        (text, padded) => {
            expect(decodeBase64Url(padded.replaceAll("=", ""))).toEqual(textBytes(text));
        },
    );

    test("decodes URL-safe alphabet output byte-exactly", () => {
        expect(decodeBase64Url("-_8")).toEqual(
            new Uint8Array([
                0xfb,
                0xff,
            ]),
        );
        expect(decodeBase64Url("_w")).toEqual(
            new Uint8Array([
                0xff,
            ]),
        );
    });

    test("rejects padding, the JWS-segment grammar carries none", () => {
        expect(() => decodeBase64Url("Zg==")).toThrow();
        expect(() => decodeBase64Url("Zm8=")).toThrow();
    });

    test("rejects the standard alphabet, the empty string, and whitespace", () => {
        expect(() => decodeBase64Url("+/8")).toThrow();
        expect(() => decodeBase64Url("")).toThrow();
        expect(() => decodeBase64Url("Zg ")).toThrow();
    });
});

describe("call-site round trips", () => {
    test("the AES credentials key encodes and decodes exactly like the stored secret", () => {
        const keyBytes = new Uint8Array(32).map((_, index) => index);
        const encoded = encodeBase64(keyBytes);
        // Pins the encoding produced for bytes 0x00..0x1f — the shape of the
        // CREDENTIALS_AES_KEY secret consumed by /common/credentials.
        expect(encoded).toBe("AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=");
        expect(decodeBase64(encoded)).toEqual(keyBytes);
    });

    test("a JWT payload segment round-trips like the OpenAI id_token parser consumes it", () => {
        const claims = {
            "https://api.openai.com/auth": {
                chatgpt_account_id: "account-1",
            },
        };
        const segment = encodeBase64Url(textBytes(JSON.stringify(claims)));
        expect(segment).toMatch(/^[A-Za-z0-9_-]+$/);
        const decoded = new TextDecoder("utf-8", {
            fatal: true,
            ignoreBOM: false,
        }).decode(decodeBase64Url(segment));
        expect(JSON.parse(decoded)).toEqual(claims);
    });

    test("OAuth state blobs and PKCE challenges round-trip as 43-character base64url", async () => {
        const state = crypto.getRandomValues(new Uint8Array(32));
        const encodedState = encodeBase64Url(state);
        expect(encodedState).toMatch(/^[A-Za-z0-9_-]{43}$/);
        expect(decodeBase64Url(encodedState)).toEqual(state);

        const verifier = encodeBase64Url(crypto.getRandomValues(new Uint8Array(32)));
        const challenge = encodeBase64Url(
            new Uint8Array(await crypto.subtle.digest("SHA-256", textBytes(verifier))),
        );
        expect(challenge).toMatch(/^[A-Za-z0-9_-]{43}$/);
        expect(decodeBase64Url(challenge)).toEqual(
            new Uint8Array(await crypto.subtle.digest("SHA-256", textBytes(verifier))),
        );
    });

    test("every length class round-trips through both codecs", () => {
        for (const length of [
            0,
            1,
            2,
            3,
            4,
            5,
            6,
            3000,
            3001,
            3002,
        ]) {
            const bytes = new Uint8Array(length).map((_, index) => index % 251);
            const base64 = encodeBase64(bytes);
            expect(base64).toHaveLength(4 * Math.ceil(length / 3));
            expect(decodeBase64(base64)).toEqual(bytes);

            const base64Url = encodeBase64Url(bytes);
            expect(base64Url).toHaveLength(4 * Math.ceil(length / 3) - ((3 - (length % 3)) % 3));
            // The empty JWS segment is not decodable; the codecs round-trip every
            // non-empty length.
            if (length === 0) {
                expect(() => decodeBase64Url(base64Url)).toThrow();
                continue;
            }
            expect(decodeBase64Url(base64Url)).toEqual(bytes);
        }
    });
});
