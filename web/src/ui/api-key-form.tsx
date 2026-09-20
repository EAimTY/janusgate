import type { JSX } from "@solidjs/web";
import { createSignal, Show } from "solid-js";
import { Button, TextField } from "/src/ui/controls";
import { Glyph } from "/src/ui/glyph";

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

/** Copies one value to the clipboard; the label confirms where it was taken. */
export function CopyButton(props: {
    readonly value: string;
    readonly isDisabled?: boolean;
    readonly label?: string | undefined;
}): JSX.Element {
    const [isCopied, setIsCopied] = createSignal(false);
    let timer: number | undefined;

    const copy = async (): Promise<void> => {
        if (props.value === "") {
            return;
        }

        try {
            if (navigator.clipboard !== undefined) {
                await navigator.clipboard.writeText(props.value);
            } else {
                const area = document.createElement("textarea");
                area.value = props.value;
                document.body.appendChild(area);
                area.select();
                document.execCommand("copy");
                area.remove();
            }

            setIsCopied(true);
            window.clearTimeout(timer);
            timer = window.setTimeout(() => setIsCopied(false), 1500);
        } catch {
            setIsCopied(false);
        }
    };

    return (
        <Button
            glyph="copy"
            title={props.label ?? "Copy to clipboard"}
            isDisabled={(props.isDisabled ?? false) || props.value === ""}
            onClick={() => void copy()}
        >
            {isCopied() ? "Copied" : (props.label ?? "Copy")}
        </Button>
    );
}

/** The human label every serving key carries; names need not be unique. */
export function ApiKeyNameField(props: {
    readonly name: string;
    readonly onNameInput: (value: string) => void;
    readonly isDisabled?: boolean;
}): JSX.Element {
    return (
        <TextField
            label="Name"
            value={props.name}
            onInput={props.onNameInput}
            isDisabled={props.isDisabled ?? false}
            isData={false}
            hint="Which client holds this key. Names need not be unique; a conflict is reported against the secret, never the name."
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
}): JSX.Element {
    return (
        <div>
            <TextField
                label="Key"
                value={props.secret}
                onInput={props.onSecretInput}
                isDisabled={props.isDisabled ?? false}
                placeholder="sk-… or Generate one"
                hint={
                    props.hint ??
                    "Sent as Authorization: Bearer <key> by serving clients. Shown in full so it can be copied into client configuration."
                }
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

/** The serving switch, shared wording wherever a key is edited. */
export function ApiKeyServingHint(): JSX.Element {
    return (
        <p class="flex items-start gap-1.5 text-[13px] text-ink-faint">
            <Glyph
                name="info"
                class="mt-0.5 shrink-0"
            />
            Disabling stops this key on the next request. With no enabled key left, the gateway
            serves nothing.
        </p>
    );
}
