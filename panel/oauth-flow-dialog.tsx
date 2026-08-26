import type { JSX } from "@solidjs/web";
import { createSignal, Show } from "solid-js";
import {
    MODEL_PREFIX_MAX_LENGTH,
    MODEL_PREFIX_PATTERN,
    PROVIDER_NAME_MAX_LENGTH,
    PROVIDER_NAME_PATTERN,
} from "/config";
import {
    errorMessage,
    type OAuthCompletionInput,
    type OAuthType,
    type PanelApiClient,
} from "./api-client";
import { buttonPrimaryClasses, checkboxClasses, Dialog, formControlClasses, Notice } from "./ui";

export interface OAuthFlowLaunch {
    type: OAuthType;
    providerName: string | null;
    existingProviderNames: ReadonlySet<string>;
}

interface OAuthFlowDialogProps {
    client: PanelApiClient;
    launch: OAuthFlowLaunch;
    onClose: () => void;
    /** Reloads the list, records the outcome, triggers discovery, and closes the dialog. */
    onCompleted: (flow: OAuthFlowLaunch) => Promise<void>;
}

export function OAuthFlowDialog(props: OAuthFlowDialogProps): JSX.Element {
    const [authorizeUrl, setAuthorizeUrl] = createSignal<string | null>(null);
    const [oauthRedirectUrl, setOAuthRedirectUrl] = createSignal("");
    const [oauthName, setOAuthName] = createSignal("");
    const [oauthPrefix, setOAuthPrefix] = createSignal("");
    const [oauthProjectId, setOAuthProjectId] = createSignal("");
    const [isOAuthPrefixSpecified, setIsOAuthPrefixSpecified] = createSignal(false);
    const [oauthError, setOAuthError] = createSignal<string | null>(null);
    const [isStartingOAuth, setIsStartingOAuth] = createSignal(true);
    const [isCompletingOAuth, setIsCompletingOAuth] = createSignal(false);

    // Runs synchronously inside the click's user-activation window: the popup must open before
    // the async authorize-URL fetch, or popup blockers reject it.
    void launch();

    async function launch(): Promise<void> {
        const popup = window.open("about:blank", "_blank");
        if (popup !== null) popup.opener = null;
        try {
            const url = await props.client.startProviderOAuth(
                props.launch.type,
                props.launch.providerName ?? undefined,
            );
            setAuthorizeUrl(url);
            if (popup !== null) popup.location.assign(url);
        } catch (caughtError) {
            popup?.close();
            setOAuthError(errorMessage(caughtError));
        } finally {
            setIsStartingOAuth(false);
        }
    }

    async function completeOAuth(event: SubmitEvent): Promise<void> {
        event.preventDefault();
        setIsCompletingOAuth(true);
        setOAuthError(null);
        try {
            const input: OAuthCompletionInput = {
                redirectUrl: oauthRedirectUrl().trim(),
            };
            if (props.launch.providerName === null) {
                if (oauthName().trim().length > 0) input.name = oauthName().trim();
                if (isOAuthPrefixSpecified()) input.modelPrefix = oauthPrefix();
            }
            if (props.launch.type === "antigravity" && oauthProjectId().trim().length > 0) {
                input.projectId = oauthProjectId().trim();
            }
            await props.client.completeProviderOAuth(input);
            await props.onCompleted(props.launch);
        } catch (caughtError) {
            setOAuthError(errorMessage(caughtError));
        } finally {
            setIsCompletingOAuth(false);
        }
    }

    return (
        <Dialog
            isOpen
            isBusy={isCompletingOAuth()}
            title={props.launch.providerName === null ? "Connect OAuth provider" : "Reauthenticate"}
            description="Authorize in the new tab, then paste the full loopback redirect URL below."
            onClose={props.onClose}
        >
            <form
                class="grid gap-[17px] px-6 pt-[22px] pb-6 [&>[role]]:mb-0 [&_label>small]:text-[10px] [&_label>small]:leading-[1.4] [&_label>small]:font-[450] [&_label>small]:text-[#7c8880]"
                onSubmit={(event) => void completeOAuth(event)}
            >
                <Show when={oauthError() !== null}>
                    <Notice kind="error">{oauthError()}</Notice>
                </Show>
                <Show when={authorizeUrl() !== null}>
                    <Notice kind="info">
                        Authorization opened in a new tab. If it was blocked, use the link below.
                    </Notice>
                    <a
                        class="w-fit text-[12px] font-bold text-[#19784e] underline-offset-[3px]"
                        href={authorizeUrl() ?? undefined}
                        target="_blank"
                        rel="noreferrer"
                    >
                        Open authorization page ↗
                    </a>
                </Show>
                <Show when={props.launch.providerName === null}>
                    <label class="grid gap-[7px] text-xs font-[720] text-[#344038]">
                        Provider name (optional)
                        <input
                            class={formControlClasses}
                            pattern={PROVIDER_NAME_PATTERN.source}
                            maxlength={PROVIDER_NAME_MAX_LENGTH}
                            placeholder="Auto-generated when blank"
                            value={oauthName()}
                            onInput={(event) => setOAuthName(event.currentTarget.value)}
                        />
                    </label>
                    <label class="flex items-center gap-[7px] text-xs font-[720] text-[#344038]">
                        <input
                            class={checkboxClasses}
                            type="checkbox"
                            checked={isOAuthPrefixSpecified()}
                            onChange={(event) =>
                                setIsOAuthPrefixSpecified(event.currentTarget.checked)
                            }
                        />
                        Set a custom model prefix
                    </label>
                    <label class="grid gap-[7px] text-xs font-[720] text-[#344038]">
                        Model prefix
                        <input
                            class={formControlClasses}
                            maxlength={MODEL_PREFIX_MAX_LENGTH}
                            pattern={MODEL_PREFIX_PATTERN.source}
                            disabled={!isOAuthPrefixSpecified()}
                            placeholder="Blank means no prefix"
                            value={oauthPrefix()}
                            onInput={(event) => setOAuthPrefix(event.currentTarget.value)}
                        />
                        <small>
                            Leave customization off for the provider-name default. Turn it on and
                            leave this blank to explicitly use no prefix.
                        </small>
                    </label>
                </Show>
                <Show when={props.launch.type === "antigravity"}>
                    <label class="grid gap-[7px] text-xs font-[720] text-[#344038]">
                        Google Cloud project ID (optional)
                        <input
                            class={formControlClasses}
                            placeholder="Required only for Workspace/project-backed accounts"
                            value={oauthProjectId()}
                            onInput={(event) => setOAuthProjectId(event.currentTarget.value)}
                        />
                        <small>
                            Leave blank for Antigravity-managed free-tier projects. Workspace
                            accounts may require their own project ID.
                        </small>
                    </label>
                </Show>
                <label class="grid gap-[7px] text-xs font-[720] text-[#344038]">
                    Full redirect URL
                    <textarea
                        class={`${formControlClasses} resize-y`}
                        required
                        rows={4}
                        spellcheck={false}
                        placeholder="http://localhost:…/auth/callback?code=…&state=…"
                        value={oauthRedirectUrl()}
                        onInput={(event) => setOAuthRedirectUrl(event.currentTarget.value)}
                    />
                </label>
                <footer class="flex items-center justify-end gap-[9px] pt-[3px] max-[540px]:flex-col-reverse max-[540px]:items-stretch max-[540px]:[&>button]:w-full">
                    <span class="mr-auto text-[10px] text-[#818c85] max-[540px]:m-0 max-[540px]:text-center">
                        {isStartingOAuth()
                            ? "Preparing authorization…"
                            : "OAuth state expires in 15 minutes."}
                    </span>
                    <button
                        class={buttonPrimaryClasses}
                        type="submit"
                        disabled={isStartingOAuth() || isCompletingOAuth()}
                    >
                        {isCompletingOAuth() ? "Completing…" : "Complete connection"}
                    </button>
                </footer>
            </form>
        </Dialog>
    );
}
