import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, Match, Show, Switch } from "solid-js";
import { connectionService } from "/src/connection/app-service";
import type { Connection } from "/src/connection/model";
import { createManagementApi } from "/src/management-api/client";
import { currentLocation, hrefs, matchAppRoute } from "/src/router";
import { ConnectionSetupScreen } from "/src/screens/connection-setup-screen";
import { ModelsScreen } from "/src/screens/models-screen";
import { NotFoundScreen } from "/src/screens/not-found-screen";
import { OauthStartScreen } from "/src/screens/oauth-start-screen";
import { ProviderCreateScreen } from "/src/screens/provider-create-screen";
import { ProviderDetailScreen } from "/src/screens/provider-detail-screen";
import { ProvidersScreen } from "/src/screens/providers-screen";
import { AppLink } from "/src/ui/app-link";
import { Button } from "/src/ui/button";

/**
 * The application gate (README §3.1): without a connection only the setup
 * screen exists; with one, the administration shell. A `keyed` boundary means
 * a different connection gets a different shell — and therefore a different
 * management API client.
 */
export function App(): JSX.Element {
    const connection = createMemo(() => {
        const state = connectionService.connectionState();
        return state.status === "connected" ? state.connection : null;
    });

    return (
        <Show
            when={connection()}
            keyed
            fallback={<ConnectionSetupScreen />}
        >
            {(connected) => <AdminShell connection={connected} />}
        </Show>
    );
}

function navLinkClass(isActive: boolean): JSX.ClassValue {
    return [
        "text-sm font-medium transition-colors",
        isActive ? "text-slate-900" : "text-slate-500 hover:text-slate-900",
    ];
}

function AdminShell(props: { readonly connection: Connection }): JSX.Element {
    const api = createManagementApi(props.connection, {
        onUnauthorized: (notice) => {
            // A late 401 answering the previous connection must not clear the
            // current one.
            if (connectionService.currentConnection() === props.connection) {
                connectionService.reportUnauthorized(notice);
            }
        },
    });

    const route = createMemo(() =>
        matchAppRoute(currentLocation().pathname, currentLocation().search),
    );

    const providerDetailRoute = createMemo(() => {
        const current = route();
        return current.name === "providerDetail" ? current : null;
    });

    const modelsRoute = createMemo(() => {
        const current = route();
        return current.name === "models" ? current : null;
    });

    const oauthRoute = createMemo(() => {
        const current = route();
        return current.name === "oauthStart" ? current : null;
    });

    const isProvidersSection = (): boolean => {
        const name = route().name;
        return name === "providers" || name === "providerCreate" || name === "providerDetail";
    };

    return (
        <div class="flex min-h-screen flex-col bg-slate-100 text-slate-900">
            <header class="sticky top-0 z-10 border-b border-slate-200 bg-white/90 backdrop-blur">
                <div class="mx-auto flex w-full max-w-5xl flex-wrap items-center gap-x-6 gap-y-2 px-4 py-3">
                    <AppLink
                        href={hrefs.providers()}
                        class="text-sm font-semibold tracking-tight text-slate-900"
                    >
                        janusgate-web
                    </AppLink>
                    <nav class="flex items-center gap-4">
                        <AppLink
                            href={hrefs.providers()}
                            class={navLinkClass(isProvidersSection())}
                            isCurrent={isProvidersSection()}
                        >
                            Providers
                        </AppLink>
                        <AppLink
                            href={hrefs.models(null)}
                            class={navLinkClass(route().name === "models")}
                            isCurrent={route().name === "models"}
                        >
                            Models
                        </AppLink>
                    </nav>
                    <div class="ml-auto flex flex-wrap items-center gap-3">
                        <span class="font-mono text-xs text-slate-500">
                            {props.connection.baseUrl}
                        </span>
                        <ForgetConnectionButton />
                    </div>
                </div>
            </header>

            <main class="mx-auto w-full max-w-5xl flex-1 px-4 py-8">
                <Switch fallback={<NotFoundScreen />}>
                    <Match when={route().name === "providers"}>
                        <ProvidersScreen api={api} />
                    </Match>
                    <Match when={route().name === "providerCreate"}>
                        <ProviderCreateScreen api={api} />
                    </Match>
                    <Match
                        when={providerDetailRoute()}
                        keyed
                    >
                        {(detail) => (
                            <ProviderDetailScreen
                                api={api}
                                providerId={detail.providerId}
                            />
                        )}
                    </Match>
                    <Match
                        when={modelsRoute()}
                        keyed
                    >
                        {(models) => (
                            <ModelsScreen
                                api={api}
                                providerFilter={models.providerFilter}
                            />
                        )}
                    </Match>
                    <Match
                        when={oauthRoute()}
                        keyed
                    >
                        {(oauth) => (
                            <OauthStartScreen
                                api={api}
                                reauthorizeProviderId={oauth.reauthorizeProviderId}
                            />
                        )}
                    </Match>
                </Switch>
            </main>
        </div>
    );
}

/** Clearing the pair is explicit and confirmed; the token must be re-entered. */
function ForgetConnectionButton(): JSX.Element {
    const [isConfirming, setIsConfirming] = createSignal(false);

    return (
        <Show
            when={isConfirming()}
            fallback={
                <Button
                    variant="secondary"
                    onClick={() => setIsConfirming(true)}
                >
                    Forget connection
                </Button>
            }
        >
            <Button
                variant="danger"
                onClick={() => connectionService.forget()}
            >
                Confirm forget
            </Button>
            <Button
                variant="secondary"
                onClick={() => setIsConfirming(false)}
            >
                Cancel
            </Button>
        </Show>
    );
}
