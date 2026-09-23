import type { JSX } from "@solidjs/web";
import { formatTimestamp } from "/src/format";
import type { StartProviderOauthResponse } from "/src/management-api/generated/v0";
import { Button, buttonClass } from "/src/ui/controls";
import { Glyph } from "/src/ui/glyph";
import { CopyButton } from "./clipboard";

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
