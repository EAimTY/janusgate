import { expect, test } from "vitest";
import { GatewayError } from "/common/errors";
import { parseJsonObject, readBoundedRequestBody } from "/common/http";

test("request limits reject an oversized Content-Length before reading", async () => {
    const request = new Request("https://example.com", {
        body: new Uint8Array([
            1,
        ]),
        headers: {
            "Content-Length": "11",
        },
        method: "POST",
    });

    await expect(readBoundedRequestBody(request, 10)).rejects.toMatchObject({
        code: "payload_too_large",
        status: 413,
    });
    expect(request.bodyUsed).toBe(false);
});

test("request limits count actual streamed bytes instead of trusting Content-Length", async () => {
    const request = new Request("https://example.com", {
        body: new Uint8Array([
            1,
            2,
            3,
            4,
        ]),
        headers: {
            "Content-Length": "2",
        },
        method: "POST",
    });

    await expect(readBoundedRequestBody(request, 3)).rejects.toMatchObject({
        code: "payload_too_large",
        status: 413,
    });
});

test("JSON parsing returns objects and maps malformed/non-object bodies to invalid_json", () => {
    expect(parseJsonObject('{"model":"gpt"}')).toEqual({
        model: "gpt",
    });
    for (const input of [
        "{",
        "[]",
        "null",
    ]) {
        expect(() => parseJsonObject(input)).toThrow(GatewayError);
        try {
            parseJsonObject(input);
        } catch (error: unknown) {
            expect(error).toMatchObject({
                code: "invalid_json",
            });
        }
    }
});
