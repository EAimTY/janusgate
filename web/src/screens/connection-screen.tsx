import type { JSX } from "@solidjs/web";
import { createSignal, Show } from "solid-js";
import { cancelConnectionProbe, connectionService } from "/src/connection/app-service";
import { createConnectionDiagnosis } from "/src/connection/diagnosis-view";
import { normalizeBaseUrl } from "/src/connection/model";
import { translation } from "/src/i18n";
import { LanguageControl } from "/src/i18n/language-control";
import { setupCorsMessage, setupTokenMessage } from "/src/i18n/rich";
import { baseUrlFailureMessage } from "/src/i18n/validation-messages";
import { CopyButton } from "/src/ui/clipboard";
import { Button, TextField } from "/src/ui/controls";
import { Notice } from "/src/ui/feedback";
import { JanusMark } from "/src/ui/glyph";
import { SecretRevealToggle } from "/src/ui/secret-reveal";

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

    const state = connectionService.connectionState;
    const isVerifying = (): boolean => state().status === "verifying";

    // Whether this setup visit's address field was seeded by the connection
    // state — e.g. a mid-session 401's dying address, a forget's served
    // address, or any refused probe's restated address.
    // The seed exists precisely so the reconnect never demands an origin
    // retype, so once one landed, the interrupted-attempt reset below keeps
    // the field's address; only a visit that never received a seed (a
    // pristine first visit) clears it. A plain variable, not a signal: it
    // guards the reset, and nothing renders from it.
    let isAddressSeeded = false;

    /**
     * Closes out the interrupted attempt: the token, the reveal state, and
     * the fields' inline errors clear at once. The address clears only when
     * nothing seeded it — exactly a pristine first visit's hand-typed
     * address — while a seeded address survives, because losing it would
     * demand the origin retype the seed exists to spare. The diagnosis is
     * the diagnosis machine's own: a cancellation's landing clears it there.
     */
    const resetForm = (): void => {
        if (!isAddressSeeded) {
            setBaseUrl("");
        }
        setToken("");
        setHasSubmitted(false);
        setIsTokenVisible(false);
    };

    /**
     * Ends the probe in flight and closes out the interrupted attempt
     * (`resetForm`). The cancelled attempt's landing is the machine's
     * cancellation — no failure, nothing to seed or diagnose from it.
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
        return normalization.ok ? null : baseUrlFailureMessage(normalization.reason);
    };

    /** The token field's inline error, open only once a submit was made. */
    const tokenError = (): string | null =>
        hasSubmitted() && token() === ""
            ? translation()(($) => $.connectionValidation.managementToken)
            : null;

    // The refused probe's diagnosis and its reachability refinement live in
    // the connection machine's own hook; the screen keeps the form it drives:
    // a refusal's or forget's address seeds the field, and a cancelled
    // attempt's landing carries no failure, so it seeds and diagnoses nothing.
    const diagnosis = createConnectionDiagnosis({
        state,
        fieldBaseUrl: baseUrl,
        onSeed: (seedBaseUrl) => {
            if (seedBaseUrl !== null) {
                setBaseUrl(seedBaseUrl);
                isAddressSeeded = true;
            }
        },
    });

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
                <div class="flex flex-wrap items-center justify-between gap-3">
                    <div class="flex items-center gap-2.5 text-accent">
                        <JanusMark size={22} />
                        <span class="text-[16px] font-semibold tracking-tight text-ink">
                            JanusGate
                        </span>
                    </div>
                    <LanguageControl />
                </div>

                <div>
                    <h1 class="text-[22px] font-semibold tracking-tight text-balance">
                        {translation()(($) => $.connection.connectToJanusGate)}
                    </h1>
                    <p class="mt-2 text-ink-dim">
                        {translation()(
                            ($) => $.connection.thisAppAdministersAJanusGateGatewayItsUpstream,
                        )}
                    </p>
                </div>

                <ol class="flex list-none flex-col gap-2.5 rounded-xl bg-canvas px-4 py-4 text-[13.5px] dark:bg-muted">
                    <RequirementStep index={1}>
                        {translation()(
                            ($) => $.connection.aRunningJanusGateInstanceReachableFromThisBrowser,
                        )}
                    </RequirementStep>
                    <RequirementStep index={2}>{setupTokenMessage()}</RequirementStep>
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
                                subject={translation()(($) => $.connection.theOriginBesideIt)}
                                ariaLabel={translation()(($) => $.connection.copyThisAppsOrigin)}
                            />
                            <span class="basis-full">{setupCorsMessage()}</span>
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
                                rawDetail={current().rawDetail}
                                fix={current().fix}
                            />
                        )}
                    </Show>

                    <TextField
                        label={translation()(($) => $.connection.gatewayAddress)}
                        value={baseUrl()}
                        onInput={setBaseUrl}
                        type="url"
                        placeholder="https://gateway.example.com"
                        autocomplete="url"
                        isDisabled={isVerifying()}
                        error={baseUrlError()}
                        hint={translation()(($) => $.connection.schemeHostAndPortOnly)}
                    />

                    <TextField
                        label={translation()(($) => $.connection.managementToken)}
                        value={token()}
                        onInput={setToken}
                        type={isTokenVisible() ? "text" : "password"}
                        autocomplete="off"
                        isDisabled={isVerifying()}
                        error={tokenError()}
                        hint={translation()(($) => $.connection.sentOnlyToThatInstanceAndKeptOnly)}
                    />

                    <SecretRevealToggle
                        caption={translation()(
                            ($) => $.screens.connectionScreen.showTheTokenWhileTyping,
                        )}
                        isVisible={isTokenVisible()}
                        onToggle={setIsTokenVisible}
                    />

                    <div class="flex flex-wrap items-center gap-4">
                        <Button
                            type="submit"
                            tone="primary"
                            isBusy={isVerifying()}
                        >
                            {isVerifying()
                                ? translation()(($) => $.screens.connectionScreen.probingTheGateway)
                                : translation()(($) => $.screens.connectionScreen.connect)}
                        </Button>
                        <Show when={isVerifying()}>
                            <Button
                                tone="ghost"
                                onClick={cancelProbe}
                            >
                                {translation()(($) => $.connection.cancel)}
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
