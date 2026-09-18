import type { JSX } from "@solidjs/web";
import { createSignal, Show } from "solid-js";
import { formatTimestamp } from "/src/format";
import type { StartProviderOauthResponse } from "/src/management-api/generated/v0";
import { Button } from "/src/ui/controls";
import { Glyph } from "/src/ui/glyph";

/** Whether the authorize address reached the clipboard, and how that went. */
type CopyState = "idle" | "copied" | "refused";

/**
 * The step between `oauth:start` and pasting the code back: the address to
 * approve access at, the single-use state that names the flow, the moment the
 * flow stops being completable, and the form the caller hangs under it. The
 * app neither stores the state and the code nor composes any address out of
 * them.
 */
export function AuthorizeStep(props: {
    readonly flow: StartProviderOauthResponse;
    readonly onStartOver: () => void;
    readonly children: JSX.Element;
}): JSX.Element {
    const [copyState, setCopyState] = createSignal<CopyState>("idle");

    const copyAuthorizeUrl = async (): Promise<void> => {
        try {
            await navigator.clipboard.writeText(props.flow.authorize_url);
            setCopyState("copied");
        } catch {
            // A browser may refuse the clipboard — an insecure context, a
            // denied permission. The flow is untouched: the address stays
            // selectable in its box, and the app says so rather than
            // pretending.
            setCopyState("refused");
        }
    };

    return (
        <div class="flex flex-col gap-4">
            <div class="rounded-lg border border-rule bg-canvas px-3 py-2.5 dark:bg-muted">
                <p class="text-[12.5px] font-medium text-ink-dim">Authorization URL</p>
                <p class="tnum mt-1.5 break-all">{props.flow.authorize_url}</p>
            </div>

            <div class="flex flex-wrap items-center gap-2">
                <a
                    href={props.flow.authorize_url}
                    target="_blank"
                    rel="noreferrer noopener"
                    class="inline-flex items-center justify-center gap-2 rounded-lg bg-accent px-3.5 py-2 text-[13.5px] font-medium text-accent-ink transition-colors duration-150 ease-out hover:bg-accent-strong"
                >
                    <Glyph name="out" />
                    Open the authorization page
                </a>
                <Button
                    glyph="copy"
                    onClick={() => void copyAuthorizeUrl()}
                >
                    Copy the URL
                </Button>
                <Show when={copyState() !== "idle"}>
                    <span
                        class={
                            copyState() === "copied"
                                ? "text-[13px] font-medium text-ok"
                                : "text-[13px] font-medium text-bad"
                        }
                        role="status"
                    >
                        {copyState() === "copied"
                            ? "Copied."
                            : "This browser refused the clipboard — select the URL above and copy it by hand."}
                    </span>
                </Show>
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
                The state is single-use, so a failed exchange needs a fresh flow. Authorization
                codes are single-use too and expire quickly: paste the code and finish promptly
                after approving access.
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
