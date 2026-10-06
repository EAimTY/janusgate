import { describe, expect, it } from "vitest";
import { modelRow, providerRow } from "/src/test-support/management-api";
import { classifyStructuralAvailability } from "./availability";

const provider = providerRow();
const model = modelRow({
    upstream_id: "M",
});

describe("classifyStructuralAvailability", () => {
    it("walks the account's existence and switch before the model is consulted", () => {
        expect(classifyStructuralAvailability(undefined, model)).toBe("account_missing");
        expect(classifyStructuralAvailability(undefined, undefined)).toBe("account_missing");
        // A disabled account classifies above the model row's existence: the
        // account rung stands first, so a disabled account over a model the
        // listing does not hold is still account_disabled.
        expect(
            classifyStructuralAvailability(
                {
                    ...provider,
                    is_enabled: false,
                },
                undefined,
            ),
        ).toBe("account_disabled");
    });

    it("classifies the model's existence, switch, and discovery state under an enabled account", () => {
        expect(classifyStructuralAvailability(provider, undefined)).toBe("model_missing");
        expect(
            classifyStructuralAvailability(provider, {
                ...model,
                is_enabled: false,
            }),
        ).toBe("model_disabled");
        expect(
            classifyStructuralAvailability(provider, {
                ...model,
                discovery_state: "vanished",
            }),
        ).toBe("model_vanished");
        expect(classifyStructuralAvailability(provider, model)).toBe("ready");
    });
});
