import { render } from "@solidjs/web";
import { describe, expect, it } from "vitest";
import {
    buttonWithText,
    createDisposals,
    paragraphWithText,
    settle,
} from "../test-support/management-api";
import { ConfirmDeleteCard } from "./confirm-delete-card";

describe("ConfirmDeleteCard state machine", () => {
    const disposes = createDisposals();

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

    it("keeps keyboard focus with the confirm — onto the armed question on arm, back to the trigger on disarm", async () => {
        const gate = mount();
        await settle();

        buttonWithText("Delete…").focus();
        buttonWithText("Delete…").click();
        await settle();

        // The armed question is where focus lands — what a screen reader
        // reads next — with the confirmation button the next Tab away.
        expect(document.activeElement).toBe(paragraphWithText("Delete alpha?"));

        buttonWithText("Keep it").click();
        await settle();
        expect(document.activeElement).toBe(buttonWithText("Delete…"));

        // A confirm that steps back — the row survived — restores the same way.
        buttonWithText("Delete…").focus();
        buttonWithText("Delete…").click();
        await settle();
        buttonWithText("Delete for good").click();
        await settle();
        gate.answer(false);
        await settle();

        expect(document.activeElement).toBe(buttonWithText("Delete…"));
    });
});
