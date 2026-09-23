import { render } from "@solidjs/web";
import { afterEach, describe, expect, it } from "vitest";
import { ConfirmDeleteCard } from "/src/ui/confirm-delete-card";

/**
 * Drains the pending promises and scheduled work of one card exchange.
 */
async function settle(): Promise<void> {
    for (let round = 0; round < 10; round += 1) {
        await new Promise<void>((resolve) => setTimeout(resolve, 0));
    }
}

function buttonWithText(text: string): HTMLButtonElement {
    for (const candidate of document.querySelectorAll("button")) {
        if (candidate.textContent?.includes(text) === true) {
            return candidate as HTMLButtonElement;
        }
    }

    throw new Error(`No button reading ${text}`);
}

describe("ConfirmDeleteCard state machine", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    /** Mounts the card over a confirm the test answers through the returned gate. */
    function mount(): {
        answer: (isDeleted: boolean) => void;
    } {
        const gate: {
            answer: (isDeleted: boolean) => void;
        } = {
            answer: () => {
                throw new Error("confirm answered before it was asked");
            },
        };

        disposes.push(
            render(
                () => (
                    <ConfirmDeleteCard
                        title="Delete this provider"
                        description="Deleting removes the provider."
                        question="Delete alpha?"
                        onConfirm={() =>
                            new Promise<boolean>((resolve) => {
                                gate.answer = resolve;
                            })
                        }
                    />
                ),
                document.body,
            ),
        );

        return gate;
    }

    it("disarms back to the armed-off state when the confirm answers that the row survived", async () => {
        const gate = mount();
        await settle();

        expect(document.body.textContent).not.toContain("Delete alpha?");

        buttonWithText("Delete…").click();
        await settle();
        expect(document.body.textContent).toContain("Delete alpha?");

        buttonWithText("Delete for good").click();
        await settle();
        expect(document.body.textContent).toContain("Deleting…");
        expect(buttonWithText("Keep it").disabled).toBe(true);

        gate.answer(false);
        await settle();

        expect(document.body.textContent).toContain("Delete…");
        expect(document.body.textContent).not.toContain("Delete alpha?");
        expect(document.body.textContent).not.toContain("Deleting…");
    });

    it("holds its post-confirm state when the confirm answers that the row is gone", async () => {
        const gate = mount();
        await settle();

        buttonWithText("Delete…").click();
        await settle();
        buttonWithText("Delete for good").click();
        await settle();
        expect(document.body.textContent).toContain("Deleting…");

        gate.answer(true);
        await settle();

        // The screen owns the navigation; the card itself neither disarms nor
        // re-arms — it stays where the confirm left it, awaiting the exit.
        expect(document.body.textContent).toContain("Delete alpha?");
        expect(document.body.textContent).not.toContain("Deleting…");
    });
});
