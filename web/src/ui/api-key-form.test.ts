import { describe, expect, it } from "vitest";
import { apiKeyNameError } from "./api-key-form";

describe("apiKeyNameError", () => {
    it("rejects only the empty name, naming the field", () => {
        const error = apiKeyNameError("");

        expect(error).toContain("Name");
    });

    it("accepts every non-empty name verbatim, whitespace included", () => {
        // The gateway's only name rule is non-empty (README §7), so trimming
        // here would reject values the gateway accepts.
        expect(apiKeyNameError(" ")).toBeNull();
        expect(apiKeyNameError("edge worker #1")).toBeNull();
    });
});
