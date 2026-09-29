import type { JSX } from "@solidjs/web";
import { createSignal, Show } from "solid-js";
import { Button } from "/src/ui/controls";
import { createInlineConfirmFocus } from "/src/ui/inline-confirm";
import { Card, CardHeader } from "/src/ui/layout";
import { InlineConfirmQuestion } from "./inline-confirm-question";

/**
 * A deletion card, confirmed where it states its blast radius: an armed
 * state asking the question, a busy deletion, and the way back. The screen
 * owns the wording and the deletion itself — `onConfirm` runs it and answers
 * whether the row is gone; the card owns everything else, disarming when the
 * row survived (the failure is reported through the screen's own channel,
 * and a deletion that navigates away takes the card with it). Focus travels
 * with the confirm: arming moves it onto the question, disarming returns it
 * to the trigger — the shared inline-confirm contract (`/src/ui/inline-confirm`).
 */
export function ConfirmDeleteCard(props: {
    readonly title: string;
    readonly description: string;
    readonly question: string;
    readonly onConfirm: () => Promise<boolean>;
}): JSX.Element {
    const [isArmed, setIsArmed] = createSignal(false);
    const [isDeleting, setIsDeleting] = createSignal(false);
    const confirmFocus = createInlineConfirmFocus();

    const remove = async (): Promise<void> => {
        if (isDeleting()) {
            return;
        }

        setIsDeleting(true);
        const isDeleted = await props.onConfirm();
        setIsDeleting(false);

        if (!isDeleted) {
            confirmFocus.disarm();
            setIsArmed(false);
        }
    };

    return (
        <Card class="border-bad/30">
            <CardHeader
                title={props.title}
                description={props.description}
            />
            <Show
                when={isArmed()}
                fallback={
                    <div
                        class="mt-4"
                        ref={confirmFocus.restoreDisarmed}
                    >
                        <Button
                            tone="danger"
                            glyph="trash"
                            onClick={() => {
                                confirmFocus.arm();
                                setIsArmed(true);
                            }}
                        >
                            Delete…
                        </Button>
                    </div>
                }
            >
                <div class="mt-4 flex flex-wrap items-center gap-2">
                    <InlineConfirmQuestion
                        question={props.question}
                        focusArmed={confirmFocus.focusArmed}
                    />
                    <Button
                        tone="danger"
                        isBusy={isDeleting()}
                        onClick={() => void remove()}
                    >
                        {isDeleting() ? "Deleting…" : "Delete for good"}
                    </Button>
                    <Button
                        isDisabled={isDeleting()}
                        onClick={() => {
                            confirmFocus.disarm();
                            setIsArmed(false);
                        }}
                    >
                        Keep it
                    </Button>
                </div>
            </Show>
        </Card>
    );
}
