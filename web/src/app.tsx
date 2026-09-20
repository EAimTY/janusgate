import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, Match, Show, Switch } from "solid-js";
import { connectionService } from "/src/connection/app-service";
import type { Connection } from "/src/connection/model";
import { formatClock } from "/src/format";
import { createManagementApi } from "/src/management-api/client";
import { refreshStamp, requestRefresh } from "/src/refresh-state";
import { currentLocation, hrefs, matchAppRoute, navigate } from "/src/router";
import { ApiKeyDetailScreen } from "/src/screens/api-key-detail-screen";
import { ApiKeyNewScreen } from "/src/screens/api-key-new-screen";
import { ApiKeysScreen } from "/src/screens/api-keys-screen";
import { ConnectionScreen } from "/src/screens/connection-screen";
import { HomeScreen } from "/src/screens/home-screen";
import { ModelDetailScreen } from "/src/screens/model-detail-screen";
import { ModelNewScreen } from "/src/screens/model-new-screen";
import { ModelsScreen } from "/src/screens/models-screen";
import { NotFoundScreen } from "/src/screens/not-found-screen";
import { ProviderDetailScreen } from "/src/screens/provider-detail-screen";
import { ProviderNewScreen } from "/src/screens/provider-new-screen";
import { ProvidersScreen } from "/src/screens/providers-screen";
import { resolvedTheme, toggleTheme } from "/src/theme";
import { AppLink } from "/src/ui/app-link";
import { joinClass } from "/src/ui/attributes";
import { Button } from "/src/ui/controls";
import { Glyph, type GlyphName, JanusMark } from "/src/ui/glyph";

/**
 * The application gate: without a connection only the setup screen exists;
 * with one, the admin shell. A `keyed` boundary means a different connection
 * gets a different shell — and therefore a different management API client —
 * so nothing one instance showed can leak into another's.
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
            fallback={<ConnectionScreen />}
        >
            {(connected) => <AdminShell connection={connected} />}
        </Show>
    );
}

type NavKey = "overview" | "providers" | "models" | "keys";

const NAV_ITEMS: ReadonlyArray<{
    readonly key: NavKey;
    readonly label: string;
    readonly glyph: GlyphName;
    readonly href: string;
}> = [
    {
        key: "overview",
        label: "Overview",
        glyph: "grid",
        href: hrefs.home(),
    },
    {
        key: "providers",
        label: "Providers",
        glyph: "plug",
        href: hrefs.providers(),
    },
    {
        key: "models",
        label: "Models",
        glyph: "stack",
        href: hrefs.models(null),
    },
    {
        key: "keys",
        label: "API keys",
        glyph: "key",
        href: hrefs.apiKeys(),
    },
];

function navKeyOfRoute(name: string): NavKey | null {
    if (name === "home") {
        return "overview";
    }

    if (name === "providers" || name === "providerNew" || name === "providerDetail") {
        return "providers";
    }

    if (name === "models" || name === "modelNew" || name === "modelDetail") {
        return "models";
    }

    if (name === "apiKeys" || name === "apiKeyNew" || name === "apiKeyDetail") {
        return "keys";
    }

    return null;
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

    const modelNewRoute = createMemo(() => {
        const current = route();
        return current.name === "modelNew" ? current : null;
    });

    const apiKeyDetailRoute = createMemo(() => {
        const current = route();
        return current.name === "apiKeyDetail" ? current : null;
    });

    const modelDetailRoute = createMemo(() => {
        const current = route();
        return current.name === "modelDetail" ? current : null;
    });

    return (
        <div class="flex min-h-screen flex-col">
            <a
                href="#main"
                class="sr-only focus:not-sr-only focus:absolute focus:top-3 focus:left-3 focus:z-20 focus:rounded-lg focus:bg-accent focus:px-3 focus:py-2 focus:text-[13px] focus:font-medium focus:text-accent-ink"
            >
                Skip to content
            </a>

            <TopBar connection={props.connection} />

            <div class="mx-auto flex w-full max-w-6xl flex-1 flex-col gap-5 px-4 pt-4 pb-10 sm:px-6">
                <ContextBar
                    connection={props.connection}
                    activeNav={navKeyOfRoute(route().name)}
                />

                <main id="main">
                    <Switch fallback={<NotFoundScreen />}>
                        <Match when={route().name === "home"}>
                            <HomeScreen api={api} />
                        </Match>
                        <Match when={route().name === "providers"}>
                            <ProvidersScreen api={api} />
                        </Match>
                        <Match when={route().name === "providerNew"}>
                            <ProviderNewScreen api={api} />
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
                            when={modelNewRoute()}
                            keyed
                        >
                            {(created) => (
                                <ModelNewScreen
                                    api={api}
                                    providerFilter={created.providerFilter}
                                />
                            )}
                        </Match>
                        <Match when={route().name === "apiKeys"}>
                            <ApiKeysScreen api={api} />
                        </Match>
                        <Match when={route().name === "apiKeyNew"}>
                            <ApiKeyNewScreen api={api} />
                        </Match>
                        <Match
                            when={apiKeyDetailRoute()}
                            keyed
                        >
                            {(detail) => (
                                <ApiKeyDetailScreen
                                    api={api}
                                    apiKeyId={detail.apiKeyId}
                                />
                            )}
                        </Match>
                        <Match
                            when={modelDetailRoute()}
                            keyed
                        >
                            {(detail) => (
                                <ModelDetailScreen
                                    api={api}
                                    providerId={detail.providerId}
                                    upstreamId={detail.upstreamId}
                                />
                            )}
                        </Match>
                    </Switch>
                </main>

                <footer class="mt-4 border-t border-rule pt-4">
                    <p class="text-[12.5px] text-ink-faint">
                        {`Connected to ${props.connection.baseUrl} — the token lives only in this browser.`}
                    </p>
                </footer>
            </div>
        </div>
    );
}

/** The product bar: identity on the left, theme and disconnect on the right. */
function TopBar(props: { readonly connection: Connection }): JSX.Element {
    void props;

    return (
        <header class="sticky top-0 z-10 border-b border-rule bg-canvas/90 backdrop-blur">
            <div class="mx-auto flex h-14 w-full max-w-6xl items-center gap-2 px-4 sm:px-6">
                <AppLink
                    href={hrefs.home()}
                    class="flex items-center gap-2 text-accent"
                >
                    <JanusMark size={19} />
                    <span class="text-[15px] font-semibold tracking-tight text-ink">JanusGate</span>
                </AppLink>

                <div class="ml-auto flex items-center gap-1">
                    <button
                        type="button"
                        title={
                            resolvedTheme() === "dark"
                                ? "Switch to light theme"
                                : "Switch to dark theme"
                        }
                        aria-label={
                            resolvedTheme() === "dark"
                                ? "Switch to light theme"
                                : "Switch to dark theme"
                        }
                        onClick={() => toggleTheme()}
                        class="grid size-9 cursor-pointer place-items-center rounded-lg text-ink-dim transition-colors duration-150 ease-out hover:bg-hover hover:text-ink"
                    >
                        <Glyph name={resolvedTheme() === "dark" ? "sun" : "moon"} />
                    </button>
                    <DisconnectControl />
                </div>
            </div>
        </header>
    );
}

/** Navigation tabs plus the live connection facts: address and freshness. */
function ContextBar(props: {
    readonly connection: Connection;
    readonly activeNav: NavKey | null;
}): JSX.Element {
    return (
        <div class="flex flex-wrap items-center gap-x-4 gap-y-3">
            <nav
                aria-label="Primary"
                class="flex items-center gap-1 rounded-xl border border-rule bg-surface p-1"
            >
                {NAV_ITEMS.map((item) => (
                    <AppLink
                        href={item.href}
                        isCurrent={props.activeNav === item.key}
                        class={joinClass(
                            "inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-[13.5px] font-medium transition-colors duration-150 ease-out",
                            props.activeNav === item.key
                                ? "bg-accent-soft text-accent dark:text-accent-strong"
                                : "text-ink-dim hover:bg-hover hover:text-ink",
                        )}
                    >
                        <Glyph name={item.glyph} />
                        {item.label}
                    </AppLink>
                ))}
            </nav>

            <div class="ml-auto flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-[12.5px] text-ink-faint">
                <span
                    class="tnum max-w-56 truncate sm:max-w-72"
                    title={props.connection.baseUrl}
                >
                    {props.connection.baseUrl}
                </span>
                <span aria-hidden="true">·</span>
                <span>
                    <Show
                        when={refreshStamp()}
                        fallback="Not read yet"
                    >
                        {(stamp) => `Updated ${formatClock(stamp())}`}
                    </Show>
                </span>
                <button
                    type="button"
                    title="Read this screen's state from the gateway again"
                    onClick={() => requestRefresh()}
                    class="inline-flex cursor-pointer items-center gap-1 rounded-md px-1.5 py-1 font-medium text-ink-dim transition-colors duration-150 ease-out hover:bg-hover hover:text-ink"
                >
                    <Glyph name="refresh" />
                    Refresh
                </button>
            </div>
        </div>
    );
}

/** Disconnecting is explicit and confirmed; the token must be re-entered. */
function DisconnectControl(): JSX.Element {
    const [isArmed, setIsArmed] = createSignal(false);

    const disarm = (): void => {
        setIsArmed(false);
    };

    const goHome = (): void => {
        disarm();
        navigate(hrefs.home());
    };

    return (
        <Show
            when={isArmed()}
            fallback={
                <Button
                    tone="ghost"
                    glyph="out"
                    onClick={() => setIsArmed(true)}
                >
                    Disconnect
                </Button>
            }
        >
            <span class="flex items-center gap-1">
                <Button
                    tone="danger"
                    onClick={() => {
                        connectionService.forget();
                        goHome();
                    }}
                >
                    Forget this token?
                </Button>
                <Button
                    tone="ghost"
                    onClick={disarm}
                >
                    Keep
                </Button>
            </span>
        </Show>
    );
}
