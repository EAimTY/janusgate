import type { JSX } from "@solidjs/web";
import { createSignal, Show } from "solid-js";
import { Button, TextField } from "/src/ui/controls";
import { createInlineConfirmFocus } from "/src/ui/inline-confirm";
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
 * What the standing text is, as the Generate confirm's question names it:
 * a creation field opens empty, so text standing there was typed, while the
 * edit form opens prefilled with the row's currently-serving secret.
 */
type GenerateConfirmSubject = "typed" | "current";

/**
 * The plaintext secret with its conveniences: Generate mints a fresh value
 * locally, Copy relays the shown value to the clipboard, and a reveal turns
 * the mask off while the operator checks a paste. Generate over an empty
 * field replaces nothing and fires at once; over a standing secret it is
 * destructive — what stands there is unrecoverable — so the first click
 * arms the same one-click confirm the delete card uses, naming what is
 * lost, and only the confirm replaces; focus travels with the confirm as
 * everywhere — the shared inline-confirm contract (`/src/ui/inline-confirm`).
 * The question's subject is the
 * caller's context (`generateConfirmSubject`): a creation field opens
 * empty, so text standing there was typed, while the edit form opens
 * prefilled with the row's currently-serving secret, nothing about which is
 * typed — its confirm names the current value its Save will retire. The
 * input is masked while typed, like every pasted secret — the reveal is the
 * field's own transient state and every mount opens masked — while the
 * listing and detail cards present the stored secret in full with a copy
 * action, where it is data, not input. The default hint states the creation
 * contract, where the gate requires a secret — a caller stating its own
 * hint (the edit form, which states the rotation cost instead) owns the
 * whole line.
 */
export function ApiKeySecretField(props: {
    readonly secret: string;
    readonly onSecretInput: (value: string) => void;
    readonly generateConfirmSubject?: GenerateConfirmSubject;
    readonly isDisabled?: boolean;
    readonly hint?: string | null | undefined;
    readonly error?: string | null | undefined;
}): JSX.Element {
    const [isGenerateArmed, setIsGenerateArmed] = createSignal(false);
    const [isSecretVisible, setIsSecretVisible] = createSignal(false);
    const confirmFocus = createInlineConfirmFocus();
    // The armed state is only ever about the standing text: a field emptied
    // by hand has nothing left to lose, so the confirm yields to the plain
    // Generate button until text stands again.
    const isStandingSecretArmed = (): boolean => isGenerateArmed() && props.secret !== "";
    // The confirm names what stands in the field; the typed value is the
    // field's own default subject, creation's contract.
    const generateConfirmSubject = (): GenerateConfirmSubject =>
        props.generateConfirmSubject ?? "typed";

    const generate = (): void => {
        props.onSecretInput(generateApiKey());
        setIsGenerateArmed(false);
    };

    return (
        <div>
            <TextField
                label="Key"
                type={isSecretVisible() ? "text" : "password"}
                value={props.secret}
                onInput={props.onSecretInput}
                isDisabled={props.isDisabled ?? false}
                autocomplete="off"
                placeholder="sk-… or Generate one"
                error={props.error}
                hint={
                    props.hint ??
                    requiredHint("Sent as Authorization: Bearer <key> by serving clients.")
                }
            />
            <label class="mt-2 flex w-fit cursor-pointer items-center gap-2 text-[13.5px] text-ink-dim">
                <input
                    type="checkbox"
                    class="size-4 accent-(--color-accent)"
                    checked={isSecretVisible()}
                    onChange={(event) => setIsSecretVisible(event.currentTarget.checked)}
                />
                Show the key while typing
            </label>
            <div class="mt-2 flex flex-wrap items-center gap-2">
                <Show
                    when={isStandingSecretArmed()}
                    fallback={
                        <span
                            class="contents"
                            ref={confirmFocus.restoreDisarmed}
                        >
                            <Button
                                glyph="key"
                                isDisabled={props.isDisabled ?? false}
                                onClick={() => {
                                    if (props.secret === "") {
                                        generate();
                                    } else {
                                        confirmFocus.arm();
                                        setIsGenerateArmed(true);
                                    }
                                }}
                            >
                                Generate
                            </Button>
                        </span>
                    }
                >
                    <p
                        class="w-full text-[13.5px] text-ink-dim"
                        tabindex={-1}
                        ref={confirmFocus.focusArmed}
                    >
                        Replace the {generateConfirmSubject()} secret?
                    </p>
                    <Button
                        tone="danger"
                        glyph="key"
                        isDisabled={props.isDisabled ?? false}
                        onClick={() => {
                            confirmFocus.disarm();
                            generate();
                        }}
                    >
                        Replace it
                    </Button>
                    <Button
                        isDisabled={props.isDisabled ?? false}
                        onClick={() => {
                            confirmFocus.disarm();
                            setIsGenerateArmed(false);
                        }}
                    >
                        Keep it
                    </Button>
                </Show>
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
