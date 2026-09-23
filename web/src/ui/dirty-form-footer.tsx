import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";
import { Button } from "/src/ui/controls";
import { Notice } from "/src/ui/feedback";

/**
 * The one footer every edit screen's settings form closes with (README §6.3):
 * the save button, the way back, and the status that says edits stand. The
 * form owns its dirt, its save, and its revert; this owns only the trio's
 * shape, byte-identical across the three edit screens.
 */
export function DirtyFormFooter(props: {
    readonly canSave: boolean;
    readonly isSaving: boolean;
    readonly isDirty: boolean;
    readonly onRevert: () => void;
}): JSX.Element {
    return (
        <div class="flex flex-wrap items-center gap-2">
            <Button
                type="submit"
                tone="primary"
                isDisabled={!props.canSave}
                isBusy={props.isSaving}
            >
                {props.isSaving ? "Saving…" : "Save changes"}
            </Button>
            <Button
                isDisabled={!props.isDirty || props.isSaving}
                onClick={props.onRevert}
            >
                Revert
            </Button>
            <Show when={props.isDirty && !props.isSaving}>
                <span
                    class="text-[13px] font-medium text-warn"
                    role="status"
                >
                    Unsaved changes
                </span>
            </Show>
        </div>
    );
}

/**
 * The notice every edit screen states while its row is held against a
 * refresh (README §6.3), parameterized by the one word that differs between
 * the screens: Save writes the edits over the gateway's version, Revert
 * picks the gateway's version up.
 */
export function HoldNotice(props: {
    readonly isHeld: boolean;
    readonly subject: string;
}): JSX.Element {
    return (
        <Show when={props.isHeld}>
            <Notice
                tone="warn"
                title={`This ${props.subject} changed on the gateway while you have unsaved edits.`}
                detail="Save writes your edits over the gateway's version; Revert picks the gateway's version up."
            />
        </Show>
    );
}
