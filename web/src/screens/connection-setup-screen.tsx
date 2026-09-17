import type { JSX } from "@solidjs/web";
import { createSignal, Show } from "solid-js";
import { connectionService } from "/src/connection/app-service";
import { normalizeBaseUrl } from "/src/connection/model";
import { Banner } from "/src/ui/banner";
import { Button } from "/src/ui/button";
import { TextField } from "/src/ui/field";

/**
 * The first screen any visitor sees (README §3.1): the pair the app needs
 * before it can administer anything. Submitting probes the candidate — only a
 * successful `providers:list` persists it.
 */
export function ConnectionSetupScreen(): JSX.Element {
    const [baseUrl, setBaseUrl] = createSignal("");
    const [token, setToken] = createSignal("");
    const [isTokenVisible, setIsTokenVisible] = createSignal(false);
    const [baseUrlError, setBaseUrlError] = createSignal<string | null>(null);
    const [tokenError, setTokenError] = createSignal<string | null>(null);

    const state = connectionService.connectionState;
    const isVerifying = (): boolean => state().status === "verifying";
    const notice = (): string | null => {
        const currentState = state();
        return currentState.status === "unconnected" ? currentState.notice : null;
    };

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
        <main class="grid min-h-screen place-items-center bg-slate-100 px-4 py-10">
            <div class="w-full max-w-lg">
                <h1 class="text-center text-2xl font-bold tracking-tight text-slate-900">
                    janusgate-web
                </h1>
                <p class="mt-2 text-center text-sm text-slate-600">
                    Enter the base URL and management token of the JanusGate instance you want to
                    administer. Both stay in this browser.
                </p>

                <form
                    novalidate={true}
                    class="mt-6 flex flex-col gap-4 rounded-lg border border-slate-200 bg-white p-6 shadow-sm"
                    onSubmit={submit}
                >
                    <Show when={notice()}>
                        {(message) => (
                            <Banner variant="error">
                                <p class="font-medium">The connection was refused.</p>
                                <p class="mt-1">{message()}</p>
                            </Banner>
                        )}
                    </Show>

                    <TextField
                        id="connection-base-url"
                        label="Management API base URL"
                        value={baseUrl()}
                        onInput={setBaseUrl}
                        placeholder="https://gateway.example.com"
                        autocomplete="url"
                        required
                        disabled={isVerifying()}
                        error={baseUrlError()}
                        hint="The instance's origin only; the management API path is fixed by the contract."
                    />

                    <TextField
                        id="connection-token"
                        label="Management token"
                        type={isTokenVisible() ? "text" : "password"}
                        value={token()}
                        onInput={setToken}
                        autocomplete="off"
                        required
                        disabled={isVerifying()}
                        error={tokenError()}
                        hint="The value of management_api.token in that instance's configuration."
                    />

                    <label class="flex items-center gap-2 text-xs text-slate-600">
                        <input
                            type="checkbox"
                            class="size-4 rounded border-slate-300 text-slate-900 focus:ring-2 focus:ring-slate-200"
                            checked={isTokenVisible()}
                            onChange={(event) => setIsTokenVisible(event.currentTarget.checked)}
                        />
                        Show the token while typing
                    </label>

                    <Button
                        type="submit"
                        disabled={isVerifying()}
                    >
                        {isVerifying() ? "Verifying…" : "Connect"}
                    </Button>
                </form>

                <p class="mt-4 text-xs text-slate-500">
                    Cross-origin calls are the normal case: the targeted instance must list this
                    page's origin in{" "}
                    <code class="font-mono">management_api.cors.allowed_origin</code>, or the
                    browser blocks the probe. The token is sent only to that instance and is never
                    written to the URL or the console.
                </p>
            </div>
        </main>
    );
}
