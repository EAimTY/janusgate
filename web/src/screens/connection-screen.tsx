import type { JSX } from "@solidjs/web";
import { createSignal, createTrackedEffect, Show } from "solid-js";
import { connectionService } from "/src/connection/app-service";
import { normalizeBaseUrl } from "/src/connection/model";
import { type Diagnosis, diagnoseFailure, probeReachability } from "/src/management-api/diagnosis";
import { Button, TextField } from "/src/ui/controls";
import { Notice } from "/src/ui/feedback";
import { JanusMark } from "/src/ui/glyph";

/**
 * First contact: what JanusGate is, what the visitor needs before connecting,
 * and — when a connection is refused — what went wrong, in the one place a
 * stranger is most likely to give up. Cross-origin is the normal case, so a
 * blocked call and a dead address are told apart by probing the origin before
 * the diagnosis is written.
 */
export function ConnectionScreen(): JSX.Element {
    const [baseUrl, setBaseUrl] = createSignal("");
    const [token, setToken] = createSignal("");
    const [isTokenVisible, setIsTokenVisible] = createSignal(false);
    const [baseUrlError, setBaseUrlError] = createSignal<string | null>(null);
    const [tokenError, setTokenError] = createSignal<string | null>(null);
    const [diagnosis, setDiagnosis] = createSignal<Diagnosis | null>(null);

    const state = connectionService.connectionState;
    const isVerifying = (): boolean => state().status === "verifying";

    createTrackedEffect(() => {
        const current = state();

        if (current.status !== "unconnected" || current.failure === null) {
            setDiagnosis(null);
            return;
        }

        const failure = current.failure;
        const attempted = failure.baseUrl ?? baseUrl();
        const context = {
            baseUrl: attempted,
            pageOrigin: window.location.origin,
            action: "connect" as const,
        };
        const initial = diagnoseFailure(failure.cause ?? failure.message, context);

        setDiagnosis(initial);

        if (initial.kind !== "no-answer") {
            return;
        }

        // The probe separates "nothing is listening" from "the browser blocked
        // the response"; it is only worth a request while the answer is open.
        let isCurrent = true;

        void probeReachability(attempted).then((reachability) => {
            if (!isCurrent) {
                return;
            }

            setDiagnosis(
                diagnoseFailure(failure.cause ?? failure.message, {
                    ...context,
                    reachability,
                }),
            );
        });

        return () => {
            isCurrent = false;
        };
    });

    const submit = (event: SubmitEvent): void => {
        event.preventDefault();

        if (isVerifying()) {
            return;
        }

        const normalization = normalizeBaseUrl(baseUrl());
        const tokenFailure = token() === "" ? "Enter the management token." : null;

        setBaseUrlError(normalization.ok ? null : normalization.reason);
        setTokenError(tokenFailure);

        if (!normalization.ok || tokenFailure !== null) {
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
                        Connect to your gateway
                    </h1>
                    <p class="mt-2 text-ink-dim">
                        JanusGate is a headless gateway: an OpenAI-compatible API in front, several
                        upstream accounts behind it — a ChatGPT or Google login, or a static key on
                        any compatible service. This app administers those accounts, their models,
                        and their credentials. The gateway itself serves no interface.
                    </p>
                </div>

                <ol class="flex list-none flex-col gap-2.5 rounded-xl bg-canvas px-4 py-4 text-[13.5px] dark:bg-muted">
                    <RequirementStep index={1}>
                        A running JanusGate instance, reachable from this browser.
                    </RequirementStep>
                    <RequirementStep index={2}>
                        Its management token: the value of{" "}
                        <span class="tnum">management_api.token</span> in that instance's
                        configuration file.
                    </RequirementStep>
                    <RequirementStep index={3}>
                        This page's origin <span class="tnum">{window.location.origin}</span> listed
                        in that instance's{" "}
                        <span class="tnum">management_api.cors.allowed_origin</span> — or the
                        gateway served from this same origin.
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
                                title={current().headline}
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
                        hint="The origin only — scheme, host and port. The management path is fixed by the contract."
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

                    <div class="flex flex-wrap items-center gap-4">
                        <Button
                            type="submit"
                            tone="primary"
                            isBusy={isVerifying()}
                        >
                            {isVerifying() ? "Probing the gateway…" : "Connect"}
                        </Button>

                        <label class="flex cursor-pointer items-center gap-2 text-[13.5px] text-ink-dim">
                            <input
                                type="checkbox"
                                class="size-4 accent-(--color-accent)"
                                checked={isTokenVisible()}
                                onChange={(event) => setIsTokenVisible(event.currentTarget.checked)}
                            />
                            Show the token while typing
                        </label>
                    </div>
                </form>

                <p class="border-t border-rule pt-4 text-[13px] text-ink-faint">
                    The probe call is the connection test: only a management call that answers
                    proves the pair, and only then is it stored. A rejected token, a blocked origin,
                    and an address nothing answers are three different failures, and this page says
                    which one you have.
                </p>
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
