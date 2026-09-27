import type { JSX } from "@solidjs/web";
import { type Accessor, onCleanup, Show } from "solid-js";
import { appConfig } from "/src/config";
import { formatTimestamp } from "/src/format";
import type { StartProviderOauthResponse } from "/src/management-api/generated/v0";
import { CopyButton } from "/src/ui/clipboard";
import { Button, buttonClass, TextField } from "/src/ui/controls";
import { Glyph } from "/src/ui/glyph";
import type { OauthFlow } from "./oauth-flow";

/**
 * The one navigation warning both live steps state beside their own
 * guidance: the flow's state lives in the mounted screen, so any in-app
 * navigation discards it — and since the approval happens in another tab,
 * stepping elsewhere in the console while authorizing is exactly the move
 * the visitor is tempted into. A tab close or a reload is the leave-guard
 * registry's to warn for (README §6.3); the in-app move is this sentence's.
 */
const NAVIGATION_WARNING =
    "Stay on this page until the flow completes — navigating within the console discards the flow.";

/**
 * The facts both flow steps print about the live flow: the moment it stops
 * being completable, read first, and the single-use state that names it —
 * demoted to a subdued line beneath, because the visitor never acts on the
 * state and must never mistake it for the thing to paste. The app neither
 * stores the state and the code nor composes any address out of them.
 */
function FlowFacts(props: { readonly flow: StartProviderOauthResponse }): JSX.Element {
    return (
        <div class="flex flex-col gap-2">
            <dl class="flex flex-wrap gap-x-8 gap-y-3">
                <div>
                    <dt class="text-[12.5px] font-medium text-ink-dim">Flow expires</dt>
                    <dd class="tnum mt-1 text-ink-dim">{formatTimestamp(props.flow.expires_at)}</dd>
                </div>
            </dl>
            <p class="text-[12px] text-ink-faint">
                Single-use state: <span class="tnum break-all">{props.flow.state}</span>
            </p>
        </div>
    );
}

/**
 * The authorization-code step between `oauth:start` and pasting the code
 * back: the address to approve access at, the single-use state that names
 * the flow, the moment the flow stops being completable, and the form the
 * caller hangs under it. The app neither stores the state and the code nor
 * composes any address out of them.
 */
export function AuthorizeStep(props: {
    readonly flow: Extract<
        StartProviderOauthResponse,
        {
            grant: "authorization_code";
        }
    >;
    readonly onStartOver: () => void;
    readonly children: JSX.Element;
}): JSX.Element {
    return (
        <div class="flex flex-col gap-4">
            <div class="rounded-lg border border-rule bg-canvas px-3 py-2.5 dark:bg-muted">
                <p class="text-[12.5px] font-medium text-ink-dim">Authorization URL</p>
                <p class="tnum mt-1.5 break-all">{props.flow.authorize_url}</p>
            </div>

            <div class="flex flex-wrap items-center gap-2">
                {/* An external page, not an in-app route — the anchor carries
                    its own target and `rel`, styled by the button grammar. */}
                <a
                    href={props.flow.authorize_url}
                    target="_blank"
                    rel="noreferrer noopener"
                    class={buttonClass("primary")}
                >
                    <Glyph name="out" />
                    Open the authorization page
                </a>
                <CopyButton
                    value={props.flow.authorize_url}
                    subject="the URL above"
                    label="Copy the URL"
                />
            </div>

            <FlowFacts flow={props.flow} />

            <p class="max-w-[70ch] text-[13px] text-ink-faint">
                The state is single-use and the flow expires quickly — finish promptly after
                approving access. {NAVIGATION_WARNING}
            </p>

            <p class="max-w-[70ch] text-[13px] text-ink-faint">
                Most logins end on a page that shows nothing — that is expected. Paste the whole
                address the browser shows. If a page opens and displays a code, paste that code
                instead.
            </p>

            <div class="h-px w-full bg-rule" />

            {props.children}

            <div>
                <Button
                    tone="ghost"
                    onClick={props.onStartOver}
                >
                    Start over
                </Button>
            </div>
        </div>
    );
}

/**
 * The completion form both OAuth cards hang under the live authorization-code
 * step: the pasted callback field and the submit button that completes the
 * flow — the step's facts and intro stand in `AuthorizeStep` above it, and
 * each card owns the layout around the form and the flow's own stated
 * failure. The submit is the flow machine's `complete`, so the paste
 * diagnosis, the busy state, and the completion failure are the machine's
 * own; the button is disabled beside them only where the caller passes a
 * gate. The form's classes stay the caller's, because the two cards cap the
 * form's width differently.
 */
export function CompletionForm(props: {
    /** The screen's flow machine the form completes and resets. */
    readonly flow: OauthFlow;
    /** The live flow narrowed to the authorization-code grant. */
    readonly startedFlow: Extract<
        StartProviderOauthResponse,
        {
            grant: "authorization_code";
        }
    >;
    /** The submit button's label while the completion call runs. */
    readonly busyLabel: string;
    /** The submit button's label while it stands ready. */
    readonly submitLabel: string;
    /**
     * Extra completion submittability beside the machine's own guards — the
     * creation card's definition, prefix, and base-URL gate. Absent, the
     * button is never disabled beyond the in-flight state.
     */
    readonly isSubmittable?: Accessor<boolean> | undefined;
    /** The form's layout classes, owned by the card whose width the form fills. */
    readonly class?: string | undefined;
}): JSX.Element {
    return (
        <form
            novalidate={true}
            class={props.class}
            onSubmit={(event) => {
                event.preventDefault();
                void props.flow.complete();
            }}
        >
            <AuthorizeStep
                flow={props.startedFlow}
                onStartOver={props.flow.reset}
            >
                <TextField
                    label="Callback URL or authorization code"
                    value={props.flow.code()}
                    onInput={props.flow.setCode}
                    placeholder="Paste the whole callback URL"
                    isDisabled={props.flow.isCompleting()}
                    error={props.flow.pasteError()}
                />
                <div>
                    <Button
                        type="submit"
                        tone="primary"
                        isDisabled={props.isSubmittable?.() === false}
                        isBusy={props.flow.isCompleting()}
                    >
                        {props.flow.isCompleting() ? props.busyLabel : props.submitLabel}
                    </Button>
                </div>
            </AuthorizeStep>
        </form>
    );
}

/**
 * The device step: the flow polls the token endpoint on its own schedule, so
 * this panel shows the verification facts to approve through and keeps
 * asking for the outcome while the flow is live. The step's asks answer
 * `"pending"` while the flow's fragment wants the loop kept armed — a plain
 * pending answer, and now a failed-but-not-gone poll under the fragment's
 * retry bound — and `"done"` once it reports a terminal state itself, for a
 * gone flow, an exhausted run of failed polls, or a disposed screen or
 * replaced flow, which is the one disposal guard (README §6.4) keeping a
 * late resolution from re-arming the timer or navigating after the visitor
 * left. Both verification URIs are optional on the wire (gateway README
 * §7.1): without either, the verification row renders not at all and the
 * user code alone stands — never an anchor whose empty href would navigate
 * this app to itself.
 */
export function DeviceStep(props: {
    readonly flow: Extract<
        StartProviderOauthResponse,
        {
            grant: "device";
        }
    >;
    readonly onComplete: () => Promise<"pending" | "done">;
    readonly onStartOver: () => void;
}): JSX.Element {
    const completeUri = (): string =>
        props.flow.verification_uri_complete ?? props.flow.verification_uri ?? "";

    let timer: ReturnType<typeof setTimeout> | null = null;

    const clearTimer = (): void => {
        if (timer !== null) {
            clearTimeout(timer);
            timer = null;
        }
    };

    const poll = (): void => {
        void props.onComplete().then((outcome) => {
            if (outcome === "done") {
                clearTimer();
                return;
            }

            timer = setTimeout(poll, appConfig.oauthDevicePollIntervalMs);
        });
    };

    // The first ask runs once the panel is up; onSettled-style cleanup stops
    // the polling when the step leaves the screen for any reason.
    void Promise.resolve().then(poll);

    onCleanup(clearTimer);

    return (
        <div class="flex flex-col gap-4">
            <div class="rounded-lg border border-rule bg-canvas px-3 py-2.5 dark:bg-muted">
                <p class="text-[12.5px] font-medium text-ink-dim">Verification code</p>
                <p class="tnum mt-1.5 flex items-center gap-3 text-[17px] font-semibold tracking-wide">
                    {props.flow.user_code}
                    <CopyButton
                        value={props.flow.user_code}
                        subject="the verification code"
                        label="Copy the code"
                    />
                </p>
            </div>

            <Show when={completeUri() !== ""}>
                <div class="flex flex-wrap items-center gap-2">
                    <a
                        href={completeUri()}
                        target="_blank"
                        rel="noreferrer noopener"
                        class={buttonClass("primary")}
                    >
                        <Glyph name="out" />
                        Open the verification page
                    </a>
                    <CopyButton
                        value={completeUri()}
                        subject="the verification URL"
                        label="Copy the URL"
                    />
                </div>
            </Show>

            <FlowFacts flow={props.flow} />

            <p class="max-w-[70ch] text-[13px] text-ink-faint">
                Approve the code at the verification page; this page keeps asking the gateway for
                the outcome and moves on as soon as the flow completes. {NAVIGATION_WARNING}
            </p>

            <div class="h-px w-full bg-rule" />

            <div>
                <Button
                    tone="ghost"
                    onClick={() => {
                        clearTimer();
                        props.onStartOver();
                    }}
                >
                    Start over
                </Button>
            </div>
        </div>
    );
}
