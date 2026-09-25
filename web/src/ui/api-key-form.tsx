import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";
import { Button, TextField } from "/src/ui/controls";
import { CopyButton } from "./clipboard";
import { requiredHint } from "./required-hint";

/**
 * Mints one serving-key secret locally: 32 random bytes, base64url without
 * padding. The gateway accepts any non-empty, unique value, so generation is
 * a typing convenience — pasting a self-made secret works the same.
 */
function generateApiKey(): string {
    const bytes = new Uint8Array(32);
    crypto.getRandomValues(bytes);

    let binary = "";

    for (const byte of bytes) {
        binary += String.fromCharCode(byte);
    }

    return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
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

/**
 * The human label every serving key carries; names need not be unique. Both
 * serving-key forms' gates demand a non-empty name, so the field states that
 * statically — a pristine creation form explains its disabled submit before
 * any interaction.
 */
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
            hint={requiredHint("Which client holds this key.")}
        />
    );
}

/**
 * The plaintext secret with its two conveniences: Generate mints a fresh
 * value locally, Copy relays the shown value to the clipboard. The secret is
 * shown in full — the listing returns it, and the operator needs it to
 * configure clients. The default hint states the creation contract, where
 * the gate requires a secret — a caller stating its own hint (the edit
 * form's write-only contract) owns the whole line.
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
                hint={
                    props.hint ??
                    requiredHint("Sent as Authorization: Bearer <key> by serving clients.")
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
                    <CopyButton
                        value={props.secret}
                        subject="the secret here"
                    />
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
            <CopyButton
                value={props.secret}
                subject="the secret here"
            />
        </div>
    );
}
