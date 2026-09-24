import type { JSX } from "@solidjs/web";
import { onCleanup } from "solid-js";
import { formatTimestamp } from "/src/format";
import type { StartProviderOauthResponse } from "/src/management-api/generated/v0";
import { Button, buttonClass } from "/src/ui/controls";
import { Glyph } from "/src/ui/glyph";
import { CopyButton } from "./clipboard";

/**
 * The authorization-code step between `oauth:start` and pasting the code
 * back: the address to approve access at, the single-use state that names
 * the flow, the moment the flow stops being completable, and the form the
 * caller hangs under it. The app neither stores the state and the code nor
 * composes any address out of them.
 */
export function AuthorizeStep(props: {
    readonly flow: Extract<StartProviderOauthResponse, { grant: "authorization_code" }>;
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

            <dl class="flex flex-wrap gap-x-8 gap-y-3">
                <div>
                    <dt class="text-[12.5px] font-medium text-ink-dim">Single-use state</dt>
                    <dd class="tnum mt-1 break-all text-ink-dim">{props.flow.state}</dd>
                </div>
                <div>
                    <dt class="text-[12.5px] font-medium text-ink-dim">Flow expires</dt>
                    <dd class="tnum mt-1 text-ink-dim">{formatTimestamp(props.flow.expires_at)}</dd>
                </div>
            </dl>

            <p class="max-w-[70ch] text-[13px] text-ink-faint">
                Both are single-use and expire quickly — finish promptly after approving access.
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
 * The device step: the flow polls the token endpoint on its own schedule, so
 * this panel shows the verification facts to approve through and keeps
 * asking for the outcome while the flow is live. A pending answer schedules
 * the next poll on the flow's stored interval; a terminal answer consumes
 * the flow — succeeded lands on the provider, failed states the recovery.
 */
export function DeviceStep(props: {
    readonly flow: Extract<StartProviderOauthResponse, { grant: "device" }>;
    readonly onComplete: () => Promise<"pending" | "done">;
    readonly onSucceeded: () => void;
    readonly onFailed: (failureCode: string, failureMessage: string) => void;
    readonly onStartOver: () => void;
}): JSX.Element {
    const completeUri = (): string =>
        props.flow.verification_uri_complete ??
        props.flow.verification_uri ??
        "";

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

            timer = setTimeout(poll, 3000);
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

            <dl class="flex flex-wrap gap-x-8 gap-y-3">
                <div>
                    <dt class="text-[12.5px] font-medium text-ink-dim">Single-use state</dt>
                    <dd class="tnum mt-1 break-all text-ink-dim">{props.flow.state}</dd>
                </div>
                <div>
                    <dt class="text-[12.5px] font-medium text-ink-dim">Flow expires</dt>
                    <dd class="tnum mt-1 text-ink-dim">{formatTimestamp(props.flow.expires_at)}</dd>
                </div>
            </dl>

            <p class="max-w-[70ch] text-[13px] text-ink-faint">
                Approve the code at the verification page; this page keeps asking the gateway for
                the outcome and moves on as soon as the flow completes.
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
