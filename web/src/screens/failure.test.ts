import { render } from "@solidjs/web";
import { describe, expect, it } from "vitest";
import { connectionService } from "/src/connection/app-service";
import { ManagementApiError } from "/src/management-api/errors";
import { describeCallFailure } from "./failure";

describe("describeCallFailure", () => {
    it("names the address the failed call retained once the connection is gone", () => {
        // The failure that cleared the connection — a 401, a disconnect —
        // leaves its in-flight siblings to fail after it: with no live
        // connection to read, the diagnosis falls back to the address the
        // failed call itself went to, never to an empty one.
        connectionService.forget();

        const diagnosis = describeCallFailure(
            new ManagementApiError("transport", "Network request failed: Failed to fetch", {
                baseUrl: "https://gateway.example.com",
            }),
        );

        const host = document.createElement("div");
        const dispose = render(() => diagnosis.title, host);
        expect(host.textContent).toContain("https://gateway.example.com");
        expect(host.textContent).not.toContain(" .");
        expect(host.querySelector("bdi")?.getAttribute("dir")).toBe("ltr");
        dispose();
    });
});
