import type { JSX } from "@solidjs/web";
import { createSignal, Show } from "solid-js";
import { Button, TextField } from "/src/ui/controls";
import { copyText } from "./clipboard";

/**
 * Mints one serving-key secret locally: 32 random bytes, base64url without
 * padding. The gateway accepts any non-empty, unique value, so generation is
 * a typing convenience — pasting a self-made secret works the same.
 */
export function generateApiKey(): string {
    const bytes = new Uint8Array(32);
    crypto.getRandomValues(bytes);

    let binary = "";

    for (const byte of bytes) {
        binary += String.fromCharCode(byte);
    }

    return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** Whether the copy attempt reached the clipboard, and how that went. */
type CopyState = "idle" | "copied" | "refused";

/**
 * Copies one value to the clipboard; the label confirms where it was taken.
 * A clipboard the browser refuses is stated — the value stays selectable
 * beside the button, so the operator can copy it by hand.
 */
export function CopyButton(props: {
    readonly value: string;
    readonly isDisabled?: boolean;
    readonly label?: string | undefined;
}): JSX.Element {
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

    return (
        <>
            <Button
                glyph="copy"
                title={props.label ?? "Copy to clipboard"}
                isDisabled={(props.isDisabled ?? false) || props.value === ""}
                onClick={() => void copy()}
            >
                {copyState() === "copied" ? "Copied" : (props.label ?? "Copy")}
            </Button>
            <Show when={copyState() === "refused"}>
                <span
                    class="text-[13px] font-medium text-bad"
                    role="status"
                >
                    This browser refused the clipboard — select the secret here and copy it by hand.
                </span>
            </Show>
        </>
    );
}

/**
 * The inline error for a serving key's name, shown where it was typed. The
 * gateway's only name rule is non-empty (gateway README §7) — every
 * non-empty value is accepted verbatim, whitespace included, so nothing is
 * trimmed here: rejecting more than the gateway would block a legal name.
 */
export function apiKeyNameError(name: string): string | null {
    return name === "" ? "Name cannot be empty." : null;
}

/** The human label every serving key carries; names need not be unique. */
export function ApiKeyNameField(props: {
    readonly name: string;
    readonly onNameInput: (value: string) => void;
    readonly isDisabled?: boolean;
    readonly error?: string | null | undefined;
}): JSX.Element {
    return (
        <TextField
            label="Name"
            value={props.name}
            onInput={props.onNameInput}
            isDisabled={props.isDisabled ?? false}
            isData={false}
            error={props.error}
            hint="Which client holds this key."
        />
    );
}

/**
 * The plaintext secret with its two conveniences: Generate mints a fresh
 * value locally, Copy relays the shown value to the clipboard. The secret is
 * shown in full — the listing returns it, and the operator needs it to
 * configure clients.
 */
export function ApiKeySecretField(props: {
    readonly secret: string;
    readonly onSecretInput: (value: string) => void;
    readonly isDisabled?: boolean;
    readonly hint?: string | null | undefined;
    readonly error?: string | null | undefined;
}): JSX.Element {
    return (
        <div>
            <TextField
                label="Key"
                value={props.secret}
                onInput={props.onSecretInput}
                isDisabled={props.isDisabled ?? false}
                placeholder="sk-… or Generate one"
                error={props.error}
                hint={props.hint ?? "Sent as Authorization: Bearer <key> by serving clients."}
            />
            <div class="mt-2 flex flex-wrap gap-2">
                <Button
                    glyph="key"
                    isDisabled={props.isDisabled ?? false}
                    onClick={() => props.onSecretInput(generateApiKey())}
                >
                    Generate
                </Button>
                <Show when={props.secret !== ""}>
                    <CopyButton value={props.secret} />
                </Show>
            </div>
        </div>
    );
}

/** The full secret as readable data with a copy action beside it. */
export function ApiKeySecretValue(props: { readonly secret: string }): JSX.Element {
    return (
        <div class="flex flex-wrap items-start gap-2">
            <p class="tnum min-w-0 flex-1 basis-56 rounded-lg border border-rule-strong bg-canvas px-3 py-2 break-all dark:bg-muted">
                {props.secret}
            </p>
            <CopyButton value={props.secret} />
        </div>
    );
}
