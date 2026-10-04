import { render } from "@solidjs/web";
import { describe, expect, it } from "vitest";
import { CompatibleConnectionFields, ProviderEnabledSwitch } from "/src/ui/provider-form";
import {
    createDisposals,
    labeledCheckbox,
    labeledInput,
    settle,
    switchLabel,
} from "../screens/screen-test-support";

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

/** Mounts the account serving switch with its label interaction. */
function mountEnabledSwitch(props: {
    readonly isDisabled?: boolean;
    readonly onToggle: (isEnabled: boolean) => void;
}): void {
    disposes.push(
        render(
            () => (
                <ProviderEnabledSwitch
                    isEnabled={false}
                    isDisabled={props.isDisabled ?? false}
                    onToggle={props.onToggle}
                />
            ),
            document.body,
        ),
    );
}

describe("CompatibleConnectionFields upstream key reveal", () => {
    it("masks the upstream key and pairs it with an unchecked reveal toggle", () => {
        mountFields();

        const keyInput = labeledInput("Upstream API key");

        expect(keyInput.type).toBe("password");
        // The address beside it is never masked.
        expect(labeledInput("Upstream base URL").type).toBe("url");
        expect(labeledCheckbox("Show the API key while typing").checked).toBe(false);
    });

    it("reveals and re-masks the pasted key through the toggle", async () => {
        mountFields();

        const toggle = labeledCheckbox("Show the API key while typing");
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

describe("ProviderEnabledSwitch label", () => {
    it("toggles the switch when its visible label is clicked", () => {
        const toggles: boolean[] = [];
        mountEnabledSwitch({
            onToggle: (isEnabled) => toggles.push(isEnabled),
        });

        switchLabel("Enabled").click();

        expect(toggles).toEqual([
            true,
        ]);
    });

    it("does not toggle the switch through its label while the switch is disabled", () => {
        const toggles: boolean[] = [];
        mountEnabledSwitch({
            isDisabled: true,
            onToggle: (isEnabled) => toggles.push(isEnabled),
        });

        switchLabel("Enabled").click();

        expect(toggles).toEqual([]);
    });
});
