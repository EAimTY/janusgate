import type { JSX } from "@solidjs/web";
import type { TFunction } from "i18next";
import { createSignal, onCleanup, Show } from "solid-js";
import { translation } from "/src/i18n";
import { Button } from "/src/ui/controls";

/**
 * Copies one value to the operating-system clipboard, reporting whether any
 * path took it. The async Clipboard API is tried first; a browser that
 * refuses it — an insecure context leaves `navigator.clipboard` undefined,
 * and a denied permission or an unfocused document rejects the write — falls
 * back to the deprecated `document.execCommand("copy")` path through a
 * transient textarea, which reports its own failure as `false`. The textarea
 * is `readonly` and pinned fixed off-screen so Safari's implicit focus on
 * `select()` cannot scroll the document to reveal it.
 */
export async function copyText(value: string): Promise<boolean> {
    if (navigator.clipboard !== undefined) {
        try {
            await navigator.clipboard.writeText(value);
            return true;
        } catch {
            // Refused — the legacy path below is the second chance.
        }
    }

    const area = document.createElement("textarea");
    area.value = value;
    // `readonly` keeps iOS Safari from opening the keyboard on the implicit
    // focus, and the fixed off-screen placement means that focus can never
    // scroll the document — fixed-position elements are unreachable by it.
    area.setAttribute("readonly", "");
    area.style.position = "fixed";
    area.style.top = "-9999px";
    area.style.left = "-9999px";
    document.body.appendChild(area);
    area.select();

    try {
        return document.execCommand("copy");
    } finally {
        area.remove();
    }
}

/** Whether the copy attempt reached the clipboard, and how that went. */
type CopyState = "idle" | "copied" | "refused";

/**
 * Copies one value to the clipboard; the label confirms where it was taken.
 * A clipboard the browser refuses is stated — the value stays selectable
 * beside the button, so the operator can copy it by hand. `subject` names
 * that selectable value in the refusal line ("the secret here", "the URL
 * above"). One rhythm for every call site: a copied label reverts shortly,
 * while a refusal stands until a later copy replaces it — the copied state
 * describes one value's trip to the clipboard and must expire (the value can
 * be regenerated under the button), but a refusal diagnoses the environment
 * and must outlive the click that surfaced it.
 */
export function CopyButton(props: {
    readonly value: string;
    /** Explicit translator for isolated previews; production uses the tracked facade. */
    readonly translator?: TFunction<"translation"> | undefined;
    readonly subject?: string | undefined;
    readonly isDisabled?: boolean;
    readonly label?: string | undefined;
    /** The button's accessible name, where several copies share one visible label. */
    readonly ariaLabel?: string | undefined;
}): JSX.Element {
    const t = (): TFunction<"translation"> => props.translator ?? translation();
    const [copyState, setCopyState] = createSignal<CopyState>("idle");
    let timer: number | undefined;

    const copy = async (): Promise<void> => {
        if (props.value === "") {
            return;
        }

        window.clearTimeout(timer);

        if (await copyText(props.value)) {
            setCopyState("copied");
            timer = window.setTimeout(() => setCopyState("idle"), 1500);
        } else {
            setCopyState("refused");
        }
    };

    // A copied label's revert must not fire after the button leaves the tree.
    onCleanup(() => window.clearTimeout(timer));

    return (
        <>
            <Button
                glyph="copy"
                title={props.label ?? t()(($) => $.additional.clipboard.copyToClipboard)}
                ariaLabel={props.ariaLabel}
                isDisabled={(props.isDisabled ?? false) || props.value === ""}
                onClick={() => void copy()}
            >
                {copyState() === "copied"
                    ? t()(($) => $.additional.clipboard.copied)
                    : (props.label ?? t()(($) => $.additional.clipboard.copy))}
            </Button>
            <Show when={copyState() === "refused"}>
                <span
                    class="text-[13px] font-medium text-bad"
                    role="status"
                >
                    {t()(($) => $.dynamicMessages.refusedClipboard, {
                        subject: props.subject ?? t()(($) => $.additional.clipboard.theValueHere),
                    })}
                </span>
            </Show>
        </>
    );
}
