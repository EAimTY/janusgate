import { expect, test } from "vitest";
import { readBoundedJsonObject, readBoundedRequestBody } from "./http";

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

test("JSON parsing returns objects and maps malformed/non-object bodies to invalid_json", async () => {
    const objectRequest = new Request("https://example.com", {
        body: '{"model":"gpt"}',
        method: "POST",
    });
    await expect(readBoundedJsonObject(objectRequest, 100)).resolves.toEqual({
        model: "gpt",
    });
    for (const input of [
        "{",
        "[]",
        "null",
    ]) {
        const request = new Request("https://example.com", {
            body: input,
            method: "POST",
        });
        await expect(readBoundedJsonObject(request, 100)).rejects.toMatchObject({
            code: "invalid_json",
        });
    }
});
