import { render } from "@solidjs/web";
import { afterEach, describe, expect, it } from "vitest";
import { CompatibleConnectionFields } from "/src/ui/provider-form";
import { settle } from "../screens/screen-test-support";

const disposes: Array<() => void> = [];

afterEach(() => {
    for (const dispose of disposes.reverse()) {
        dispose();
    }
    disposes.length = 0;
});

/** Mounts the static-key connection fields holding a pasted upstream key. */
function mountFields(): void {
    disposes.push(
        render(
            () => (
                <CompatibleConnectionFields
                    hasBaseUrl={true}
                    baseUrl="https://upstream.example.com/v1"
                    apiKey="sk-upstream-123"
                    onBaseUrlInput={() => undefined}
                    onApiKeyInput={() => undefined}
                />
            ),
            document.body,
        ),
    );
}

/** The input the given field label names, through the label's `for` pairing. */
function inputWithLabel(label: string): HTMLInputElement {
    const owner = Array.from(document.querySelectorAll("label")).find(
        (candidate) => candidate.textContent === label,
    );

    if (owner === undefined) {
        throw new Error(`No field labelled ${label}`);
    }

    const control = document.getElementById(owner.htmlFor);

    if (!(control instanceof HTMLInputElement)) {
        throw new Error(`No input labelled ${label}`);
    }

    return control;
}

/** The checkbox the given label text wraps. */
function checkboxWithLabel(label: string): HTMLInputElement {
    const owner = Array.from(document.querySelectorAll("label")).find(
        (candidate) => candidate.textContent === label,
    );

    if (owner === undefined) {
        throw new Error(`No checkbox labelled ${label}`);
    }

    const control = owner.querySelector("input");

    if (!(control instanceof HTMLInputElement) || control.type !== "checkbox") {
        throw new Error(`No checkbox labelled ${label}`);
    }

    return control;
}

describe("CompatibleConnectionFields upstream key reveal", () => {
    it("masks the upstream key and pairs it with an unchecked reveal toggle", () => {
        mountFields();

        const keyInput = inputWithLabel("API key");

        expect(keyInput.type).toBe("password");
        // The address beside it is never masked.
        expect(inputWithLabel("Upstream base URL").type).toBe("url");
        expect(checkboxWithLabel("Show the API key while typing").checked).toBe(false);
    });

    it("reveals and re-masks the pasted key through the toggle", async () => {
        mountFields();

        const toggle = checkboxWithLabel("Show the API key while typing");
        const keyInput = inputWithLabel("API key");

        toggle.click();
        await settle();
        expect(keyInput.type).toBe("text");
        // The paste survives the reveal: confirming it is the toggle's point.
        expect(keyInput.value).toBe("sk-upstream-123");

        toggle.click();
        await settle();
        expect(keyInput.type).toBe("password");
    });
});
