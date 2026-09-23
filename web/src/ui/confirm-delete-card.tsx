import type { JSX } from "@solidjs/web";
import { createSignal, Show } from "solid-js";
import { Button } from "/src/ui/controls";
import { Card, CardHeader } from "/src/ui/layout";

/**
 * A deletion card, confirmed where it states its blast radius: an armed
 * state asking the question, a busy deletion, and the way back. The screen
 * owns the wording and the deletion itself — `onConfirm` runs it and answers
 * whether the row is gone; the card owns everything else, disarming when the
 * row survived (the failure is reported through the screen's own channel,
 * and a deletion that navigates away takes the card with it).
 */
export function ConfirmDeleteCard(props: {
    readonly title: string;
    readonly description: string;
    readonly question: string;
    readonly onConfirm: () => Promise<boolean>;
}): JSX.Element {
    const [isArmed, setIsArmed] = createSignal(false);
    const [isDeleting, setIsDeleting] = createSignal(false);

    const remove = async (): Promise<void> => {
        if (isDeleting()) {
            return;
        }

        setIsDeleting(true);
        const isDeleted = await props.onConfirm();
        setIsDeleting(false);

        if (!isDeleted) {
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
                    <div class="mt-4">
                        <Button
                            tone="danger"
                            glyph="trash"
                            onClick={() => setIsArmed(true)}
                        >
                            Delete…
                        </Button>
                    </div>
                }
            >
                <div class="mt-4 flex flex-wrap items-center gap-2">
                    <p class="w-full text-[13.5px] text-ink-dim">{props.question}</p>
                    <Button
                        tone="danger"
                        isBusy={isDeleting()}
                        onClick={() => void remove()}
                    >
                        {isDeleting() ? "Deleting…" : "Delete for good"}
                    </Button>
                    <Button
                        isDisabled={isDeleting()}
                        onClick={() => setIsArmed(false)}
                    >
                        Keep it
                    </Button>
                </div>
            </Show>
        </Card>
    );
}
