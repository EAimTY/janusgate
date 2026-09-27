import { render } from "@solidjs/web";
import { createSignal } from "solid-js";
import { afterEach, describe, expect, it } from "vitest";
import { buttonWithText, labeledInput, settle, typeInto } from "../screens/screen-test-support";
import { ApiKeySecretField, apiKeyNameError } from "./api-key-form";

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

/** The shape of a locally minted secret: 32 random bytes, base64url unpadded. */
function isGeneratedSecret(value: string): boolean {
    return /^[A-Za-z0-9_-]{43}$/.test(value);
}

describe("ApiKeySecretField Generate guard", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    /** Mounts the field over a signal, so the parent's state is observable. */
    function mountField(initialSecret: string): {
        readonly secret: () => string;
    } {
        const [secret, setSecret] = createSignal(initialSecret);

        disposes.push(
            render(
                () => (
                    <ApiKeySecretField
                        secret={secret()}
                        onSecretInput={setSecret}
                    />
                ),
                document.body,
            ),
        );

        return {
            secret,
        };
    }

    it("generates straight into an empty field", async () => {
        const field = mountField("");
        await settle();

        buttonWithText("Generate").click();
        await settle();

        expect(isGeneratedSecret(labeledInput("Key").value)).toBe(true);
        expect(isGeneratedSecret(field.secret())).toBe(true);
    });

    it("arms a confirm over a typed secret instead of replacing it", async () => {
        mountField("");
        await settle();

        typeInto(labeledInput("Key"), "sk-hand-made");
        await settle();
        buttonWithText("Generate").click();
        await settle();

        expect(labeledInput("Key").value).toBe("sk-hand-made");
        expect(document.body.textContent).toContain("Replace the typed secret?");
    });

    it("replaces the typed secret only on the confirm's click", async () => {
        mountField("");
        await settle();

        typeInto(labeledInput("Key"), "sk-hand-made");
        await settle();
        buttonWithText("Generate").click();
        await settle();
        buttonWithText("Replace it").click();
        await settle();

        expect(isGeneratedSecret(labeledInput("Key").value)).toBe(true);
        expect(document.body.textContent).not.toContain("Replace the typed secret?");
    });

    it("keeps the typed secret and disarms on Keep it", async () => {
        mountField("");
        await settle();

        typeInto(labeledInput("Key"), "sk-hand-made");
        await settle();
        buttonWithText("Generate").click();
        await settle();
        buttonWithText("Keep it").click();
        await settle();

        expect(labeledInput("Key").value).toBe("sk-hand-made");
        expect(document.body.textContent).not.toContain("Replace the typed secret?");
        expect(buttonWithText("Generate")).toBeDefined();
    });

    it("disarms once the field empties by hand, generating directly again", async () => {
        mountField("");
        await settle();

        typeInto(labeledInput("Key"), "sk-hand-made");
        await settle();
        buttonWithText("Generate").click();
        await settle();
        typeInto(labeledInput("Key"), "");
        await settle();

        expect(document.body.textContent).not.toContain("Replace the typed secret?");

        buttonWithText("Generate").click();
        await settle();

        expect(isGeneratedSecret(labeledInput("Key").value)).toBe(true);
    });
});
