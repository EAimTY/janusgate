import { describe, expect, it } from "vitest";
import { relayNativeStreamingResponse } from "./upstream-response";

describe("relayNativeStreamingResponse", () => {
    it("preserves status, safe headers, and body bytes", async () => {
        const bytes = new Uint8Array([
            0,
            1,
            2,
            255,
        ]);
        const upstream = new Response(bytes, {
            headers: {
                "Content-Type": "application/octet-stream",
                "Retry-After": "12",
                "Set-Cookie": "must-not-leak=1",
            },
            status: 429,
        });
        const result = await relayNativeStreamingResponse(upstream, new AbortController());

        expect(result.status).toBe(429);
        expect(result.headers.get("Content-Type")).toBe("application/octet-stream");
        expect(result.headers.get("Retry-After")).toBe("12");
        expect(result.headers.has("Set-Cookie")).toBe(false);
        expect(new Uint8Array(await result.arrayBuffer())).toEqual(bytes);
    });
});
