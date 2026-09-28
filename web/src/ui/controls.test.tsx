import { render } from "@solidjs/web";
import { createSignal } from "solid-js";
import { describe, expect, it } from "vitest";
import { createDisposals, drainWrites, optionRepr } from "/src/screens/screen-test-support";
import { SelectField } from "/src/ui/controls";

const disposes = createDisposals();

/** The one select the mounts below render. */
function theSelect(): HTMLSelectElement {
    const control = document.querySelector("select");

    if (!(control instanceof HTMLSelectElement)) {
        throw new Error("No select rendered");
    }

    return control;
}

describe("SelectField", () => {
    it("follows a value change over a stable options array, without any option turning over", async () => {
        const options = [
            {
                value: "alpha",
                label: "Alpha",
            },
            {
                value: "beta",
                label: "Beta",
            },
        ];
        const [value, setValue] = createSignal("alpha");

        disposes.push(
            render(
                () => (
                    <SelectField
                        label="Pick"
                        value={value()}
                        options={options}
                        onChange={setValue}
                    />
                ),
                document.body,
            ),
        );

        const select = theSelect();
        expect(select.value).toBe("alpha");

        // The invariant the option-churn design hid: the displayed selection
        // is the select's own state, so it moves with the value even when no
        // option node is recreated.
        const firstOption = select.options.item(0);

        if (firstOption === null) {
            throw new Error("No options rendered");
        }

        firstOption.dataset["pinned"] = "1";

        setValue("beta");
        await drainWrites();

        expect(select.value).toBe("beta");
        expect(firstOption.dataset["pinned"]).toBe("1");
    });

    it("renders a value no option carries as its trailing selected option, labeled with the value by default", async () => {
        const options = [
            {
                value: "alpha",
                label: "Alpha",
            },
        ];
        const [value, setValue] = createSignal("gone");

        disposes.push(
            render(
                () => (
                    <SelectField
                        label="Pick"
                        value={value()}
                        options={options}
                        onChange={setValue}
                    />
                ),
                document.body,
            ),
        );

        const select = theSelect();
        const optionReprs = Array.from(select.options).map(optionRepr);

        expect(optionReprs).toEqual([
            {
                value: "alpha",
                label: "Alpha",
                isSelected: false,
            },
            {
                value: "gone",
                label: "gone",
                isSelected: true,
            },
        ]);

        // The absent value becoming listed retires the trailing option.
        setValue("alpha");
        await drainWrites();

        expect(Array.from(select.options).map((option) => option.value)).toEqual([
            "alpha",
        ]);
    });
});
