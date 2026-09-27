import type { JSX } from "@solidjs/web";
import { createEffect, createSignal, Show } from "solid-js";
import {
    ConnectionProbeCancelled,
    cancelConnectionProbe,
    connectionService,
    PROBE_TIMEOUT_MS,
} from "/src/connection/app-service";
import { normalizeBaseUrl } from "/src/connection/model";
import { type Diagnosis, diagnoseFailure, probeReachability } from "/src/management-api/diagnosis";
import { CopyButton } from "/src/ui/clipboard";
import { Button, TextField } from "/src/ui/controls";
import { Notice } from "/src/ui/feedback";
import { JanusMark } from "/src/ui/glyph";

/**
 * First contact: what JanusGate is, what the visitor needs before connecting,
 * and — when a connection is refused — what went wrong, in the one place a
 * stranger is most likely to give up. Cross-origin is the normal case, so a
 * blocked call and a dead address are told apart by probing the origin before
 * the diagnosis is written — except the one pairing the browser refuses
 * outright (an HTTPS page calling an HTTP address), which is diagnosed
 * without issuing any probe.
 */
export function ConnectionScreen(): JSX.Element {
    const pageOrigin = window.location.origin;
    const [baseUrl, setBaseUrl] = createSignal("");
    const [token, setToken] = createSignal("");
    const [isTokenVisible, setIsTokenVisible] = createSignal(false);
    // The fields' inline errors open behind the first submit, then follow
    // their field's current value: a pristine form is not an error — the
    // hints state the requirements quietly — and an edit that fixes a field
    // retracts its error without another submit (the app's lazy-error
    // convention).
    const [hasSubmitted, setHasSubmitted] = createSignal(false);
    const [diagnosis, setDiagnosis] = createSignal<Diagnosis | null>(null);

    const state = connectionService.connectionState;
    const isVerifying = (): boolean => state().status === "verifying";

    /**
     * Returns the form to its pristine state: both fields empty, no inline
     * errors, no diagnosis — exactly what a first visit renders.
     */
    const resetForm = (): void => {
        setBaseUrl("");
        setToken("");
        setHasSubmitted(false);
        setIsTokenVisible(false);
        setDiagnosis(null);
    };

    /**
     * Ends the probe in flight and reopens the form pristine. The fields
     * empty at once so the interrupted attempt is gone from the screen
     * immediately; the aborted probe's landing is then recognized by its
     * cause below, so no failure is ever seeded from a cancelled attempt.
     */
    const cancelProbe = (): void => {
        resetForm();
        cancelConnectionProbe();
    };

    /** The address field's inline error, open only once a submit was made. */
    const baseUrlError = (): string | null => {
        if (!hasSubmitted()) {
            return null;
        }

        const normalization = normalizeBaseUrl(baseUrl());
        return normalization.ok ? null : normalization.reason;
    };

    /** The token field's inline error, open only once a submit was made. */
    const tokenError = (): string | null =>
        hasSubmitted() && token() === "" ? "Enter the management token." : null;

    createEffect(
        () => {
            const current = state();
            return current.status === "unconnected" ? current : null;
        },
        (unconnected) => {
            // A cancelled probe lands here like any refusal — the service
            // records every probe outcome — but cancellation is control
            // flow, never a failure: the interrupted form reopens pristine,
            // bypassing the seeding below entirely. The recognition is the
            // landing's cause, so a genuine refusal after the cancel seeds
            // exactly as it always did.
            if (unconnected?.failure?.cause instanceof ConnectionProbeCancelled) {
                resetForm();
                return;
            }

            // The refusal seeds from the address its failure carried — a
            // mid-session 401's, so the field reopens with it and a later
            // edit of the field cannot misattribute the failure — and a
            // deliberate forget seeds from the address the connection
            // served, so the standard token rotation (disconnect, forget,
            // paste, connect) never demands an origin retype. The token is
            // never carried or seeded: it is a bearer secret only the user
            // can re-enter.
            const seedBaseUrl =
                unconnected?.failure?.baseUrl ?? unconnected?.forgottenBaseUrl ?? null;

            if (seedBaseUrl !== null) {
                setBaseUrl(seedBaseUrl);
            }

            const failure = unconnected?.failure ?? null;

            if (failure === null) {
                // Connected, verifying, pristine, or freshly forgotten: a
                // forget diagnoses nothing, because nothing was refused.
                setDiagnosis(null);
                return;
            }

            const attempted = failure.baseUrl ?? baseUrl();
            const context = {
                baseUrl: attempted,
                pageOrigin,
                action: "connect" as const,
            };
            const initial = diagnoseFailure(failure.cause ?? failure.message, context);

            setDiagnosis(initial);

            if (initial.kind !== "no-answer") {
                return;
            }

            // The probe separates "nothing is listening" from "the browser blocked
            // the response"; it is only worth a request while the answer is open.
            // It carries the connection probe's own deadline, so a blackhole
            // address cannot leave the refinement pending minutes after the
            // screen gave up; the flag still discards an answer that outlives
            // this diagnosis.
            let isCurrent = true;

            void probeReachability(attempted, AbortSignal.timeout(PROBE_TIMEOUT_MS)).then(
                (reachability) => {
                    if (!isCurrent) {
                        return;
                    }

                    setDiagnosis(
                        diagnoseFailure(failure.cause ?? failure.message, {
                            ...context,
                            reachability,
                        }),
                    );
                },
            );

            return () => {
                isCurrent = false;
            };
        },
    );

    const submit = (event: SubmitEvent): void => {
        event.preventDefault();

        if (isVerifying()) {
            return;
        }

        // The first submit opens the fields' error gate; from then on each
        // error follows its field's value until a fix retracts it.
        setHasSubmitted(true);

        const normalization = normalizeBaseUrl(baseUrl());

        if (!normalization.ok || token() === "") {
            return;
        }

        void connectionService.connect({
            baseUrl: normalization.baseUrl,
            token: token(),
        });
    };

    return (
        <div class="grid min-h-screen place-items-center px-4 py-10">
            <div class="rise-in flex w-full max-w-xl flex-col gap-6 rounded-2xl border border-rule bg-surface px-6 py-7 sm:px-8">
                <div class="flex items-center gap-2.5 text-accent">
                    <JanusMark size={22} />
                    <span class="text-[16px] font-semibold tracking-tight text-ink">JanusGate</span>
                </div>

                <div>
                    <h1 class="text-[22px] font-semibold tracking-tight text-balance">
                        Connect to JanusGate
                    </h1>
                    <p class="mt-2 text-ink-dim">
                        This app administers a JanusGate gateway: its upstream accounts, their
                        models, and their credentials.
                    </p>
                </div>

                <ol class="flex list-none flex-col gap-2.5 rounded-xl bg-canvas px-4 py-4 text-[13.5px] dark:bg-muted">
                    <RequirementStep index={1}>
                        A running JanusGate instance, reachable from this browser.
                    </RequirementStep>
                    <RequirementStep index={2}>
                        Its management token: <span class="tnum">management_api.token</span> in that
                        instance's configuration file.
                    </RequirementStep>
                    <RequirementStep index={3}>
                        <span class="flex flex-wrap items-center gap-x-4 gap-y-2">
                            <span
                                class="tnum min-w-0 truncate text-[13px]"
                                title={pageOrigin}
                            >
                                {pageOrigin}
                            </span>
                            <CopyButton
                                value={pageOrigin}
                                subject="the origin beside it"
                                ariaLabel="Copy this app's origin"
                            />
                            <span class="basis-full">
                                in that instance's{" "}
                                <span class="tnum">management_api.cors.allowed_origin</span> —
                                unless this app is served from the gateway itself.
                            </span>
                        </span>
                    </RequirementStep>
                </ol>

                <form
                    novalidate={true}
                    class="flex flex-col gap-4"
                    onSubmit={submit}
                >
                    <Show when={diagnosis()}>
                        {(current) => (
                            <Notice
                                tone="bad"
                                title={current().title}
                                detail={current().detail}
                                fix={current().fix}
                            />
                        )}
                    </Show>

                    <TextField
                        label="Gateway address"
                        value={baseUrl()}
                        onInput={setBaseUrl}
                        type="url"
                        placeholder="https://gateway.example.com"
                        autocomplete="url"
                        isDisabled={isVerifying()}
                        error={baseUrlError()}
                        hint="Scheme, host and port only."
                    />

                    <TextField
                        label="Management token"
                        value={token()}
                        onInput={setToken}
                        type={isTokenVisible() ? "text" : "password"}
                        autocomplete="off"
                        isDisabled={isVerifying()}
                        error={tokenError()}
                        hint="Sent only to that instance, and kept only in this browser."
                    />

                    <label class="flex w-fit cursor-pointer items-center gap-2 text-[13.5px] text-ink-dim">
                        <input
                            type="checkbox"
                            class="size-4 accent-(--color-accent)"
                            checked={isTokenVisible()}
                            onChange={(event) => setIsTokenVisible(event.currentTarget.checked)}
                        />
                        Show the token while typing
                    </label>

                    <div class="flex flex-wrap items-center gap-4">
                        <Button
                            type="submit"
                            tone="primary"
                            isBusy={isVerifying()}
                        >
                            {isVerifying() ? "Probing the gateway…" : "Connect"}
                        </Button>
                        <Show when={isVerifying()}>
                            <Button
                                tone="ghost"
                                onClick={cancelProbe}
                            >
                                Cancel
                            </Button>
                        </Show>
                    </div>
                </form>
            </div>
        </div>
    );
}

function RequirementStep(props: {
    readonly index: number;
    readonly children: JSX.Element;
}): JSX.Element {
    return (
        <li class="flex gap-3">
            <span class="grid size-5 shrink-0 place-items-center rounded-full bg-accent-soft text-[12px] font-semibold text-accent dark:text-accent-strong">
                {props.index}
            </span>
            <span class="text-ink-dim">{props.children}</span>
        </li>
    );
}
