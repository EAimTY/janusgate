import type { JSX } from "@solidjs/web";
import { render } from "@solidjs/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
    Button,
    entitySelectOptions,
    listingNeedle,
    SearchInput,
    SelectField,
    Switch,
    TextField,
} from "/src/ui/controls";

let disposeView: (() => void) | null = null;

function mount(view: () => JSX.Element): HTMLElement {
    const host = document.createElement("div");
    document.body.appendChild(host);
    disposeView = render(view, host);
    return host;
}

function requireElement<TElement extends Element>(host: ParentNode, selectors: string): TElement {
    const element = host.querySelector<TElement>(selectors);
    if (element === null) {
        throw new Error(`Missing element for "${selectors}".`);
    }

    return element;
}

function setInputValue(element: HTMLInputElement | HTMLSelectElement, value: string): void {
    element.value = value;
    element.dispatchEvent(
        new Event("input", {
            bubbles: true,
        }),
    );
}

afterEach(() => {
    disposeView?.();
    disposeView = null;
    document.body.replaceChildren();
});

describe("TextField", () => {
    it("labels its input and states an untouched control as valid", () => {
        const host = mount(() => (
            <TextField
                label="Gateway address"
                value=""
                onInput={vi.fn()}
            />
        ));

        const input = requireElement<HTMLInputElement>(host, "input");
        const label = requireElement<HTMLLabelElement>(host, "label");
        expect(label.getAttribute("for")).toBe(input.id);

        input.value = "https://gateway.example.com";
        input.dispatchEvent(
            new Event("input", {
                bubbles: true,
            }),
        );
        expect(input.getAttribute("aria-invalid")).toBe("false");
    });

    it("routes typed values through onInput", () => {
        const onInput = vi.fn<(value: string) => void>();
        const host = mount(() => (
            <TextField
                label="Name"
                value=""
                onInput={onInput}
            />
        ));

        const input = requireElement<HTMLInputElement>(host, "input");
        setInputValue(input, "staging");
        expect(onInput).toHaveBeenCalledWith("staging");
    });

    it("announces a standing error ahead of the hint and marks the control invalid", () => {
        const host = mount(() => (
            <TextField
                label="Name"
                value=""
                onInput={vi.fn()}
                hint="Which client holds this key."
                error="Name cannot be empty."
            />
        ));

        const input = requireElement<HTMLInputElement>(host, "input");
        const error = requireElement<HTMLParagraphElement>(host, "p[role='alert']");
        const hint = requireElement<HTMLParagraphElement>(host, "p:not([role])");
        expect(input.getAttribute("aria-invalid")).toBe("true");
        expect(input.getAttribute("aria-describedby")).toBe(`${error.id} ${hint.id}`);
        expect(error.textContent).toContain("Name cannot be empty.");
    });

    it("describes nothing while neither error nor hint stands", () => {
        const host = mount(() => (
            <TextField
                label="Name"
                value=""
                onInput={vi.fn()}
            />
        ));

        const input = requireElement<HTMLInputElement>(host, "input");
        expect(host.querySelector("p[role='alert']")).toBeNull();
        expect(input.getAttribute("aria-describedby")).toBeNull();
    });

    it("renders the password type it is given", () => {
        const host = mount(() => (
            <TextField
                label="Token"
                value=""
                onInput={vi.fn()}
                type="password"
            />
        ));

        expect(requireElement<HTMLInputElement>(host, "input").type).toBe("password");
    });
});

describe("SelectField", () => {
    it("selects the listed value and reports the picked option", () => {
        const onChange = vi.fn<(value: string) => void>();
        const host = mount(() => (
            <SelectField
                label="Provider"
                value="a"
                options={[
                    {
                        value: "a",
                        label: "Alpha",
                    },
                    {
                        value: "b",
                        label: "Beta",
                    },
                ]}
                onChange={onChange}
            />
        ));

        const select = requireElement<HTMLSelectElement>(host, "select");
        expect(select.value).toBe("a");
        expect(select.options.length).toBe(2);

        select.value = "b";
        select.dispatchEvent(
            new Event("change", {
                bubbles: true,
            }),
        );
        expect(onChange).toHaveBeenCalledWith("b");
    });

    it("keeps stating an unlisted value through a trailing option worded by absentLabel", () => {
        const host = mount(() => (
            <SelectField
                label="Provider"
                value="99"
                options={[
                    {
                        value: "a",
                        label: "Alpha",
                    },
                ]}
                absentLabel={(value) => `${value} (no longer listed)`}
                onChange={vi.fn<(value: string) => void>()}
            />
        ));

        const select = requireElement<HTMLSelectElement>(host, "select");
        expect(select.options.length).toBe(2);
        const trailing = select.options[1];
        expect(trailing?.value).toBe("99");
        expect(trailing?.textContent).toBe("99 (no longer listed)");
        expect(trailing?.selected).toBe(true);
        expect(select.value).toBe("99");
    });

    it("falls back to the value itself as the trailing option's label", () => {
        const host = mount(() => (
            <SelectField
                label="Window"
                value="48h"
                options={[
                    {
                        value: "24h",
                        label: "Last 24 hours",
                    },
                ]}
                onChange={vi.fn<(value: string) => void>()}
            />
        ));

        const select = requireElement<HTMLSelectElement>(host, "select");
        const trailing = select.options[1];
        expect(trailing?.textContent).toBe("48h");
    });
});

describe("entitySelectOptions", () => {
    it("opens with the absent head entry and carries each row id as an exact decimal string", () => {
        const options = entitySelectOptions(
            [
                {
                    id: 7,
                    name: "Alpha",
                },
                {
                    id: 12,
                    name: "Beta",
                },
            ],
            (row) => row.name,
            "All providers",
        );

        expect(options).toEqual([
            {
                value: "",
                label: "All providers",
            },
            {
                value: "7",
                label: "Alpha",
            },
            {
                value: "12",
                label: "Beta",
            },
        ]);
    });
});

describe("Switch", () => {
    it("mirrors its state in aria-checked and reports toggles", () => {
        let isOn = false;
        const host = mount(() => (
            <Switch
                label="Enabled"
                isOn={isOn}
                onToggle={(next) => {
                    isOn = next;
                }}
            />
        ));

        const knob = requireElement<HTMLButtonElement>(host, "button[role='switch']");
        expect(knob.getAttribute("aria-checked")).toBe("false");
        expect(knob.getAttribute("aria-label")).toBe("Enabled");

        knob.click();
        expect(isOn).toBe(true);
    });

    it("describes itself with its hint and stays inert while disabled", () => {
        const onToggle = vi.fn<(isOn: boolean) => void>();
        const host = mount(() => (
            <Switch
                label="Enabled"
                hint="A disabled switch serves nothing."
                isOn={true}
                isDisabled={true}
                onToggle={onToggle}
            />
        ));

        const knob = requireElement<HTMLButtonElement>(host, "button[role='switch']");
        expect(knob.getAttribute("aria-checked")).toBe("true");
        expect(knob.disabled).toBe(true);
        const hint = requireElement<HTMLParagraphElement>(host, "p");
        expect(knob.getAttribute("aria-describedby")).toBe(hint.id);

        knob.click();
        expect(onToggle).not.toHaveBeenCalled();
    });
});

describe("Button", () => {
    it("reports clicks and stays clickable while enabled", () => {
        const onClick = vi.fn<() => void>();
        const host = mount(() => <Button onClick={onClick}>Save changes</Button>);

        const button = requireElement<HTMLButtonElement>(host, "button");
        expect(button.type).toBe("button");
        button.click();
        expect(onClick).toHaveBeenCalledTimes(1);
    });

    it("disables while busy and submits when told to", () => {
        const onClick = vi.fn<() => void>();
        const host = mount(() => (
            <Button
                type="submit"
                isBusy={true}
                onClick={onClick}
            >
                Saving…
            </Button>
        ));

        const button = requireElement<HTMLButtonElement>(host, "button");
        expect(button.type).toBe("submit");
        expect(button.disabled).toBe(true);
        button.click();
        expect(onClick).not.toHaveBeenCalled();
    });
});

describe("SearchInput", () => {
    it("names itself for assistive technology and routes typing through onInput", () => {
        const onInput = vi.fn<(value: string) => void>();
        const host = mount(() => (
            <SearchInput
                label="Search API keys"
                placeholder="Search by name or key"
                value=""
                onInput={onInput}
            />
        ));

        const input = requireElement<HTMLInputElement>(host, "input");
        expect(input.getAttribute("aria-label")).toBe("Search API keys");
        expect(host.querySelector("label")).toBeNull();

        setInputValue(input, "staging");
        expect(onInput).toHaveBeenCalledWith("staging");
    });
});

describe("listingNeedle", () => {
    it("trims and case-folds, and an empty needle narrows nothing", () => {
        expect(listingNeedle("  Staging KEY ")).toBe("staging key");
        expect(listingNeedle("   ")).toBe("");
    });
});
