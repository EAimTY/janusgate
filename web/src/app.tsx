import type { JSX } from "@solidjs/web";
import {
    type Accessor,
    createEffect,
    createMemo,
    createSignal,
    Match,
    onCleanup,
    onSettled,
    Show,
    Switch,
} from "solid-js";
import { connectionService } from "/src/connection/app-service";
import type { Connection } from "/src/connection/model";
import { hasDirtyForms } from "/src/dirty-form-registry";
import { formatClock } from "/src/format";
import { createManagementApi } from "/src/management-api/client";
import { refreshStamp, requestRefresh, resetRefreshStamp } from "/src/refresh-state";
import {
    type AppRoute,
    currentLocation,
    hrefs,
    matchAppRoute,
    navigate,
    routeTitle,
} from "/src/router";
import { ApiKeyDetailScreen } from "/src/screens/api-key-detail-screen";
import { ApiKeyEditScreen } from "/src/screens/api-key-edit-screen";
import { ApiKeyNewScreen } from "/src/screens/api-key-new-screen";
import { ApiKeysScreen } from "/src/screens/api-keys-screen";
import { ConnectionScreen } from "/src/screens/connection-screen";
import { HomeScreen } from "/src/screens/home-screen";
import { ModelDetailScreen } from "/src/screens/model-detail-screen";
import { ModelEditScreen } from "/src/screens/model-edit-screen";
import { ModelNewScreen } from "/src/screens/model-new-screen";
import { ModelsScreen } from "/src/screens/models-screen";
import { NotFoundScreen } from "/src/screens/not-found-screen";
import { ProviderDetailScreen } from "/src/screens/provider-detail-screen";
import { ProviderEditScreen } from "/src/screens/provider-edit-screen";
import { ProviderNewScreen } from "/src/screens/provider-new-screen";
import { ProvidersScreen } from "/src/screens/providers-screen";
import { UsageQueryScreen } from "/src/screens/usage-query-screen";
import { UsageScreen } from "/src/screens/usage-screen";
import { cycleTheme, type ThemePreference, themePreference } from "/src/theme";
import { AppLink } from "/src/ui/app-link";
import { joinClass } from "/src/ui/attributes";
import { Button } from "/src/ui/controls";
import { Glyph, type GlyphName, JanusMark } from "/src/ui/glyph";

/**
 * The connection screen's document title: the one page outside the routes,
 * shown whenever no connection exists to route for.
 */
const CONNECTION_TITLE = "Connect to JanusGate";

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

    // The leave guard is the app root's own lifecycle: installed once for the
    // page's lifetime here — not per connection in the shell — and removed
    // only when the root itself goes away. It reads the dirty-form registry
    // at event time, so screens arm and disarm it purely by reporting their
    // dirt; the connection screen, which mounts no tracked form, never trips
    // it (README §6.3).
    window.addEventListener("beforeunload", guardUnsavedFormsOnLeave);
    onCleanup(() => {
        window.removeEventListener("beforeunload", guardUnsavedFormsOnLeave);
    });

    // Unconnected, the connection screen is the whole document and names it;
    // once a connection exists the shell's own effect takes the title over.
    // The null guard keeps the write on the transition into setup, so it can
    // never race the shell's route title.
    createEffect(
        () => (connection() === null ? CONNECTION_TITLE : null),
        (title) => {
            if (title !== null) {
                document.title = title;
            }
        },
    );

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

/**
 * The leave-event surface the guard acts on: the two members the standard
 * cancel pattern writes. `BeforeUnloadEvent` satisfies it structurally, so
 * the installed listener hands the real event over, and the suite pins the
 * decision on a plain stand-in instead of dispatching an event jsdom cannot
 * express.
 */
type LeaveEvent = {
    readonly preventDefault: () => void;
    returnValue: string;
};

/**
 * The one `beforeunload` guard (README §6.3): while a form the app tracks as
 * dirty stands dirty, a leave of the whole document — a tab close, a reload,
 * a navigation off the page — is marked for the browser's native leave
 * confirmation instead of silently discarding the edits. A clean registry
 * cancels nothing: the guard reads the registry live at event time, so one
 * permanent listener follows every screen's reports. In-app navigation is
 * deliberately not guarded — the nav tabs and the router navigate freely;
 * protecting it is a deferred variant, as are the creation screens whose
 * forms the app does not yet track.
 */
export function guardUnsavedFormsOnLeave(event: LeaveEvent): void {
    if (!hasDirtyForms()) {
        return;
    }

    // The standard cancel pattern: preventDefault per the spec, and the
    // legacy `returnValue` flag the engines require to actually ask.
    event.preventDefault();
    event.returnValue = "";
}

type NavKey = "overview" | "providers" | "models" | "keys" | "usage";

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
    {
        key: "usage",
        label: "Usage",
        glyph: "activity",
        href: hrefs.usage(),
    },
];

function navKeyOfRoute(name: string): NavKey | null {
    if (name === "home") {
        return "overview";
    }

    if (
        name === "providers" ||
        name === "providerNew" ||
        name === "providerDetail" ||
        name === "providerEdit"
    ) {
        return "providers";
    }

    if (
        name === "models" ||
        name === "modelNew" ||
        name === "modelDetail" ||
        name === "modelEdit"
    ) {
        return "models";
    }

    if (
        name === "apiKeys" ||
        name === "apiKeyNew" ||
        name === "apiKeyDetail" ||
        name === "apiKeyEdit"
    ) {
        return "keys";
    }

    if (name === "usage" || name === "usageQuery") {
        return "usage";
    }

    return null;
}

/**
 * The current route when it carries the named variant's payload, else `null`.
 * The equality check alone cannot narrow through the generic name, so a type
 * predicate carries the narrowing.
 */
function routeNamed<TName extends AppRoute["name"]>(
    route: Accessor<AppRoute>,
    name: TName,
): Accessor<Extract<
    AppRoute,
    {
        name: TName;
    }
> | null> {
    const isNamed = (
        candidate: AppRoute,
    ): candidate is Extract<
        AppRoute,
        {
            name: TName;
        }
    > => candidate.name === name;

    return createMemo(() => {
        const current = route();
        return isNamed(current) ? current : null;
    });
}

function AdminShell(props: { readonly connection: Connection }): JSX.Element {
    // The shell is created per connection: the freshness stamp belongs to the
    // instance the reads answer, so a different connection starts unrefreshed
    // instead of showing the previous instance's stamp until its first read
    // completes. The shell also always starts at the top — the setup screen
    // can leave the document scrolled when its form was the last thing on a
    // short viewport.
    resetRefreshStamp();

    onSettled(() => {
        window.scrollTo(0, 0);
    });

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

    // The document title follows the route, so a tab, a history entry, a
    // bookmark, and a screen-reader page announcement all name the screen —
    // and the row the route addresses — that they hold.
    createEffect(
        () => routeTitle(route()),
        (title) => {
            document.title = title;
        },
    );

    const providerDetailRoute = routeNamed(route, "providerDetail");
    const providerEditRoute = routeNamed(route, "providerEdit");
    const modelsRoute = routeNamed(route, "models");
    const modelNewRoute = routeNamed(route, "modelNew");
    const apiKeyDetailRoute = routeNamed(route, "apiKeyDetail");
    const apiKeyEditRoute = routeNamed(route, "apiKeyEdit");
    const modelDetailRoute = routeNamed(route, "modelDetail");
    const modelEditRoute = routeNamed(route, "modelEdit");
    const usageRoute = routeNamed(route, "usage");
    const usageQueryRoute = routeNamed(route, "usageQuery");

    return (
        <div class="flex min-h-screen flex-col">
            <a
                href="#main"
                class="sr-only focus:not-sr-only focus:absolute focus:top-3 focus:left-3 focus:z-20 focus:rounded-lg focus:bg-accent focus:px-3 focus:py-2 focus:text-[13px] focus:font-medium focus:text-accent-ink"
            >
                Skip to content
            </a>

            <TopBar />

            <div class="mx-auto flex w-full max-w-6xl flex-1 flex-col gap-5 px-4 pt-4 pb-10 sm:px-6">
                <ContextBar
                    connection={props.connection}
                    activeNav={navKeyOfRoute(route().name)}
                    hasScreenReads={isRefreshableRoute(route())}
                />

                <main id="main">
                    <Switch fallback={<NotFoundScreen />}>
                        <Match when={route().name === "home"}>
                            <HomeScreen
                                api={api}
                                baseUrl={props.connection.baseUrl}
                            />
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
                            when={providerEditRoute()}
                            keyed
                        >
                            {(edit) => (
                                <ProviderEditScreen
                                    api={api}
                                    providerId={edit.providerId}
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
                                    upstreamId={created.upstreamId}
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
                            when={apiKeyEditRoute()}
                            keyed
                        >
                            {(edit) => (
                                <ApiKeyEditScreen
                                    api={api}
                                    apiKeyId={edit.apiKeyId}
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
                                    modelId={detail.modelId}
                                />
                            )}
                        </Match>
                        <Match
                            when={modelEditRoute()}
                            keyed
                        >
                            {(edit) => (
                                <ModelEditScreen
                                    api={api}
                                    modelId={edit.modelId}
                                />
                            )}
                        </Match>
                        <Match
                            when={usageRoute()}
                            keyed
                        >
                            {(usage) => (
                                <UsageScreen
                                    api={api}
                                    filter={usage.filter}
                                />
                            )}
                        </Match>
                        <Match
                            when={usageQueryRoute()}
                            keyed
                        >
                            {(query) => (
                                <UsageQueryScreen
                                    api={api}
                                    subject={query.subject}
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

/**
 * The theme control's face per state of the preference: the glyph draws the
 * state in force and the label names what one click does next, since the
 * cycle light → dark → system is fixed — each state can state its own
 * successor.
 */
const THEME_CONTROL_STATES: Record<
    ThemePreference,
    {
        readonly glyph: GlyphName;
        readonly label: string;
    }
> = {
    light: {
        glyph: "sun",
        label: "Light theme — switch to dark",
    },
    dark: {
        glyph: "moon",
        label: "Dark theme — switch to system",
    },
    system: {
        glyph: "display",
        label: "System theme — switch to light",
    },
};

/** The product bar: identity on the left, theme and disconnect on the right. */
function TopBar(): JSX.Element {
    const themeControl = () => THEME_CONTROL_STATES[themePreference()];

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
                        title={themeControl().label}
                        aria-label={themeControl().label}
                        onClick={() => cycleTheme()}
                        class="grid size-9 cursor-pointer place-items-center rounded-lg text-ink-dim transition-colors duration-150 ease-out hover:bg-hover hover:text-ink"
                    >
                        <Glyph name={themeControl().glyph} />
                    </button>
                    <DisconnectControl />
                </div>
            </div>
        </header>
    );
}

/**
 * The routes whose screen performs no gateway read at all, so the Refresh
 * control would pulse a refresh nothing answers: there it is disabled, its
 * hint stating why, instead of pretending. Today's two such screens are the
 * new API key page, which renders from local state only, and the not-found
 * page, the fallback for addresses the router cannot match — including a row
 * address whose identifier does not parse — which renders links alone. Every
 * other screen's reads ride the refresh pulse, the new provider page's boot
 * read included. A future screen that reads nothing joins this set with its
 * route.
 */
const REFRESHLESS_ROUTE_NAMES: ReadonlySet<AppRoute["name"]> = new Set([
    "apiKeyNew",
    "notFound",
]);

/** Whether the route's screen exposes a gateway read for the Refresh control to re-run. */
function isRefreshableRoute(route: AppRoute): boolean {
    return !REFRESHLESS_ROUTE_NAMES.has(route.name);
}

/** Navigation tabs plus the live connection facts: address and freshness. */
function ContextBar(props: {
    readonly connection: Connection;
    readonly activeNav: NavKey | null;
    /** Whether the mounted screen reads the gateway, so Refresh has a read to re-run. */
    readonly hasScreenReads: boolean;
}): JSX.Element {
    return (
        <div class="flex flex-wrap items-center gap-x-4 gap-y-3">
            <nav
                aria-label="Primary"
                class="flex flex-wrap items-center gap-1 rounded-xl border border-rule bg-surface p-1"
            >
                {NAV_ITEMS.map((item) => (
                    <AppLink
                        href={item.href}
                        isCurrent={props.activeNav === item.key}
                        class={joinClass(
                            "inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-[13.5px] font-medium whitespace-nowrap transition-colors duration-150 ease-out",
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
                        fallback="Not refreshed yet"
                    >
                        {(stamp) => `Updated ${formatClock(stamp())}`}
                    </Show>
                </span>
                <button
                    type="button"
                    title={
                        props.hasScreenReads
                            ? "Read this screen's state from the gateway again"
                            : "This screen reads nothing from the gateway, so there is nothing to refresh"
                    }
                    disabled={!props.hasScreenReads}
                    onClick={() => requestRefresh()}
                    class="inline-flex cursor-pointer items-center gap-1 rounded-md px-1.5 py-1 font-medium text-ink-dim transition-colors duration-150 ease-out hover:bg-hover hover:text-ink disabled:cursor-not-allowed disabled:opacity-50"
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
                    Forget token
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
