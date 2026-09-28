import { render } from "@solidjs/web";
import { describe, expect, it } from "vitest";
import { modelPrefixError } from "/src/management-api/identifiers";
import { CompatibleConnectionFields, ProviderIdentityFields } from "/src/ui/provider-form";
import { createDisposals, labeledInput, settle } from "../screens/screen-test-support";

const disposes = createDisposals();

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
    readonly isDisabled?: boolean;
    readonly onCustomPrefixToggle?: (isCustom: boolean) => void;
}): void {
    disposes.push(
        render(
            () => (
                <ProviderIdentityFields
                    name={props.name}
                    namePlaceholder=""
                    onNameInput={() => undefined}
                    isCustomPrefix={props.isCustomPrefix}
                    onCustomPrefixToggle={props.onCustomPrefixToggle ?? (() => undefined)}
                    customPrefix=""
                    onCustomPrefixInput={() => undefined}
                    defaultPrefix={`${props.name}-`}
                    prefixError={props.prefixError}
                    isDisabled={props.isDisabled ?? false}
                />
            ),
            document.body,
        ),
    );
}

describe("ProviderIdentityFields derived-prefix error", () => {
    it("names the derivation and its escapes while the custom field stays hidden", async () => {
        // The default prefix is the free-form name's shadow (`${name}-`), so
        // a name holding a space closes every submit gate while no prefix
        // input is mounted to state why (README §4.5): with the toggle off
        // the alert must carry the derivation and both escapes — the rule's
        // own field is not on the page, so the field-naming message would
        // name a control the operator never opened. The toggle — not the
        // error — stays the only thing that exposes the custom field.
        mountIdentityFields({
            name: "has space",
            isCustomPrefix: false,
            prefixError: modelPrefixError("has space-"),
        });
        await settle();

        expect(document.body.textContent).toContain(
            `The default prefix is the provider's name plus a dash, and the name's characters make it unroutable — a prefix accepts letters, numbers, and symbols only, with no spaces, non-ASCII text, "/", or ":". Rename the provider without them, or turn on Custom model prefix and set one.`,
        );
        expect(document.body.textContent).not.toContain("Model prefix accepts letters");
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

        const keyInput = labeledInput("Upstream API key");

        expect(keyInput.type).toBe("password");
        // The address beside it is never masked.
        expect(labeledInput("Upstream base URL").type).toBe("url");
        expect(checkboxWithLabel("Show the API key while typing").checked).toBe(false);
    });

    it("reveals and re-masks the pasted key through the toggle", async () => {
        mountFields();

        const toggle = checkboxWithLabel("Show the API key while typing");
        const keyInput = labeledInput("Upstream API key");

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

/** The label element the given switch's visible text stands in. */
function switchLabel(label: string): HTMLLabelElement {
    const owner = Array.from(document.querySelectorAll("label")).find(
        (candidate) => candidate.textContent === label,
    );

    if (owner === undefined) {
        throw new Error(`No switch label ${label}`);
    }

    return owner;
}

describe("ProviderIdentityFields switch label", () => {
    it("toggles the switch when its visible label is clicked", () => {
        const toggles: boolean[] = [];
        mountIdentityFields({
            name: "upstream",
            isCustomPrefix: false,
            prefixError: null,
            onCustomPrefixToggle: (isCustom) => toggles.push(isCustom),
        });

        switchLabel("Custom model prefix").click();

        expect(toggles).toEqual([
            true,
        ]);
    });

    it("does not toggle the switch through its label while the switch is disabled", () => {
        const toggles: boolean[] = [];
        mountIdentityFields({
            name: "upstream",
            isCustomPrefix: false,
            prefixError: null,
            isDisabled: true,
            onCustomPrefixToggle: (isCustom) => toggles.push(isCustom),
        });

        switchLabel("Custom model prefix").click();

        expect(toggles).toEqual([]);
    });
});
