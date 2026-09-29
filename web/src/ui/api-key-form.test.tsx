import { render } from "@solidjs/web";
import { createSignal } from "solid-js";
import { describe, expect, it } from "vitest";
import {
    buttonWithText,
    createDisposals,
    labeledCheckbox,
    labeledInput,
    paragraphWithText,
    settle,
    typeInto,
} from "../screens/screen-test-support";
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

const disposes = createDisposals();

/** Mounts the field over a signal, so the parent's state is observable. The
 * confirm subject defaults to the creation context's "typed", the reveal
 * label to creation's "Show the key while typing". */
function mountField(
    initialSecret: string,
    generateConfirmSubject: "typed" | "current" = "typed",
    revealLabel?: string,
): {
    readonly secret: () => string;
} {
    const [secret, setSecret] = createSignal(initialSecret);

    disposes.push(
        render(
            () => (
                <ApiKeySecretField
                    secret={secret()}
                    onSecretInput={setSecret}
                    generateConfirmSubject={generateConfirmSubject}
                    revealLabel={revealLabel}
                />
            ),
            document.body,
        ),
    );

    return {
        secret,
    };
}

describe("ApiKeySecretField masking", () => {
    it("opens masked, stays masked while typed, and reveals only on the checkbox", async () => {
        mountField("");
        await settle();

        expect(labeledInput("Key").getAttribute("type")).toBe("password");

        typeInto(labeledInput("Key"), "sk-hand-made");
        await settle();
        expect(labeledInput("Key").getAttribute("type")).toBe("password");

        labeledCheckbox("Show the key while typing").click();
        await settle();
        expect(labeledInput("Key").getAttribute("type")).toBe("text");

        labeledCheckbox("Show the key while typing").click();
        await settle();
        expect(labeledInput("Key").getAttribute("type")).toBe("password");
    });

    it("labels the reveal as the caller's context names it", async () => {
        // The edit form opens prefilled with the currently-serving secret —
        // the reveal is how the operator inspects that live value before
        // rotating — so its label names the current key, not the act of
        // typing.
        mountField("sk-serving-secret", "current", "Show the current key");
        await settle();

        expect(document.body.textContent).not.toContain("Show the key while typing");

        labeledCheckbox("Show the current key").click();
        await settle();
        expect(labeledInput("Key").getAttribute("type")).toBe("text");

        labeledCheckbox("Show the current key").click();
        await settle();
        expect(labeledInput("Key").getAttribute("type")).toBe("password");
    });
});

describe("ApiKeySecretField Generate guard", () => {
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

    it("names the standing secret as the caller's subject names it", async () => {
        // The edit form opens prefilled with the row's currently-serving
        // secret — nothing was typed — so its confirm must name the current
        // value the save will retire, not a typed one.
        mountField("sk-serving-secret", "current");
        await settle();

        buttonWithText("Generate").click();
        await settle();

        expect(labeledInput("Key").value).toBe("sk-serving-secret");
        expect(document.body.textContent).toContain("Replace the current secret?");
        expect(document.body.textContent).not.toContain("Replace the typed secret?");
    });

    it("replaces the typed secret only on the confirm's click", async () => {
        mountField("");
        await settle();

        typeInto(labeledInput("Key"), "sk-hand-made");
        await settle();
        buttonWithText("Generate").focus();
        buttonWithText("Generate").click();
        await settle();
        buttonWithText("Replace it").click();
        await settle();

        expect(isGeneratedSecret(labeledInput("Key").value)).toBe(true);
        expect(document.body.textContent).not.toContain("Replace the typed secret?");
        // The confirm's action steps back to the trigger with focus — the
        // operator can generate again without re-tabbing the page.
        expect(document.activeElement).toBe(buttonWithText("Generate"));
    });

    it("keeps the typed secret and disarms on Keep it", async () => {
        mountField("");
        await settle();

        typeInto(labeledInput("Key"), "sk-hand-made");
        await settle();
        buttonWithText("Generate").focus();
        buttonWithText("Generate").click();
        await settle();
        // The armed question takes focus — the announcement that a question
        // replaced the Generate button.
        expect(document.activeElement).toBe(paragraphWithText("Replace the typed secret?"));

        buttonWithText("Keep it").click();
        await settle();

        expect(document.activeElement).toBe(buttonWithText("Generate"));
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
