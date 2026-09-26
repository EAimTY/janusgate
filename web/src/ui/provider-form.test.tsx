import { render } from "@solidjs/web";
import { afterEach, describe, expect, it } from "vitest";
import { modelPrefixError } from "/src/management-api/identifiers";
import { CompatibleConnectionFields, ProviderIdentityFields } from "/src/ui/provider-form";
import { labeledInput, settle } from "../screens/screen-test-support";

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

/** Mounts the identity fields holding the given prefix verdict. */
function mountIdentityFields(props: {
    readonly name: string;
    readonly isCustomPrefix: boolean;
    readonly prefixError: string | null;
}): void {
    disposes.push(
        render(
            () => (
                <ProviderIdentityFields
                    name={props.name}
                    namePlaceholder=""
                    onNameInput={() => undefined}
                    isCustomPrefix={props.isCustomPrefix}
                    onCustomPrefixToggle={() => undefined}
                    customPrefix=""
                    onCustomPrefixInput={() => undefined}
                    defaultPrefix={`${props.name}-`}
                    prefixError={props.prefixError}
                />
            ),
            document.body,
        ),
    );
}

describe("ProviderIdentityFields derived-prefix error", () => {
    it("names an invalid derived prefix while the custom field stays hidden", async () => {
        // The default prefix is the free-form name's shadow (`${name}-`), so
        // a name holding a space closes every submit gate while no prefix
        // input is mounted to state why (README §4.5): the error must render
        // with the toggle off, and the toggle — not the error — stays the
        // only thing that exposes the custom field.
        mountIdentityFields({
            name: "has space",
            isCustomPrefix: false,
            prefixError: modelPrefixError("has space-"),
        });
        await settle();

        expect(document.body.textContent).toContain(
            'Model prefix accepts letters, numbers, and symbols only — no spaces, non-ASCII text, "/", or ":".',
        );
        expect(
            Array.from(document.querySelectorAll("label")).find(
                (candidate) => candidate.textContent === "Model prefix",
            ),
        ).toBeUndefined();
    });
});

describe("CompatibleConnectionFields upstream key reveal", () => {
    it("masks the upstream key and pairs it with an unchecked reveal toggle", () => {
        mountFields();

        const keyInput = labeledInput("API key");

        expect(keyInput.type).toBe("password");
        // The address beside it is never masked.
        expect(labeledInput("Upstream base URL").type).toBe("url");
        expect(checkboxWithLabel("Show the API key while typing").checked).toBe(false);
    });

    it("reveals and re-masks the pasted key through the toggle", async () => {
        mountFields();

        const toggle = checkboxWithLabel("Show the API key while typing");
        const keyInput = labeledInput("API key");

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
