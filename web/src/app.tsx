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
import { connectionService } from "./connection/app-service";
import type { Connection } from "./connection/model";
import { formatRefreshStamp } from "./format";
import { translation } from "./i18n";
import { LanguageControl } from "./i18n/language-control";
import { connectedMessage } from "./i18n/rich";
import { hasGuardedSubjects } from "./leave-guard-registry";
import { createManagementApi } from "./management-api/client";
import { refreshStamp, requestRefresh, resetRefreshStamp } from "./refresh-state";
import {
    type AppRoute,
    currentLocation,
    hrefs,
    matchAppRoute,
    navigate,
    routeTitle,
} from "./router";
import { ApiKeyDetailScreen } from "./screens/api-keys/api-key-detail-screen";
import { ApiKeyEditScreen } from "./screens/api-keys/api-key-edit-screen";
import { ApiKeyNewScreen } from "./screens/api-keys/api-key-new-screen";
import { ApiKeysScreen } from "./screens/api-keys/api-keys-screen";
import { ConnectionScreen } from "./screens/connection-screen";
import { HomeScreen } from "./screens/home-screen";
import { BulkPublicationScreen } from "./screens/model-routes/bulk-publication-screen";
import { ModelRouteEditorScreen } from "./screens/model-routes/model-route-editor-screen";
import { ModelRoutesScreen } from "./screens/model-routes/model-routes-screen";
import { ModelDetailScreen } from "./screens/models/model-detail-screen";
import { ModelEditScreen } from "./screens/models/model-edit-screen";
import { ModelNewScreen } from "./screens/models/model-new-screen";
import { ModelsScreen } from "./screens/models/models-screen";
import { NotFoundScreen } from "./screens/not-found-screen";
import { ProviderPoolEditorScreen } from "./screens/provider-pools/provider-pool-editor-screen";
import { ProviderPoolsScreen } from "./screens/provider-pools/provider-pools-screen";
import { ProviderDetailScreen } from "./screens/providers/provider-detail-screen";
import { ProviderEditScreen } from "./screens/providers/provider-edit-screen";
import { ProviderNewScreen } from "./screens/providers/provider-new-screen";
import { ProvidersScreen } from "./screens/providers/providers-screen";
import { UsageQueryScreen } from "./screens/usage/usage-query-screen";
import { UsageScreen } from "./screens/usage/usage-screen";
import { cycleTheme, type ThemePreference, themePreference } from "./theme";
import { AppLink } from "./ui/app-link";
import { joinClass } from "./ui/attributes";
import { Button } from "./ui/controls";
import { Glyph, type GlyphName, JanusMark } from "./ui/glyph";
import { createInlineConfirmFocus } from "./ui/inline-confirm";
import { InlineConfirmQuestion } from "./ui/inline-confirm-question";

/**
 * The connection screen's document title: the one page outside the routes,
 * shown whenever no connection exists to route for.
 */
function connectionTitle(): string {
    return translation()(($) => $.connection.connectToJanusGate);
}

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
    // only when the root itself goes away. It reads the leave-guard registry
    // at event time, so screens arm and disarm it purely by reporting their
    // subjects' state; the connection screen, which mounts no tracked form,
    // never trips it (README §6.3).
    window.addEventListener("beforeunload", guardLeaveOnGuardedSubjects);
    onCleanup(() => {
        window.removeEventListener("beforeunload", guardLeaveOnGuardedSubjects);
    });

    // Unconnected, the connection screen is the whole document and names it;
    // once a connection exists the shell's own effect takes the title over.
    // The null guard keeps the write on the transition into setup, so it can
    // never race the shell's route title.
    createEffect(
        () => (connection() === null ? connectionTitle() : null),
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
 * cancel pattern writes. `BeforeUnloadEvent` satisfies it structurally, so the
 * installed listener hands the real event over without the guard depending on
 * a constructor the DOM does not expose.
 */
type LeaveEvent = {
    readonly preventDefault: () => void;
    returnValue: string;
};

/**
 * The one `beforeunload` guard (README §6.3): while a subject the registry
 * seats stands guarded — a form tracked as dirty, or a live OAuth flow — a
 * leave of the whole document — a tab close, a reload, a navigation off the
 * page — is marked for the browser's native leave confirmation instead of
 * silently discarding what the subject holds. A clean registry cancels
 * nothing: the guard reads the registry live at event time, so one permanent
 * listener follows every screen's reports. In-app navigation is deliberately
 * not guarded — the nav tabs and the router navigate freely; protecting it
 * is a deferred variant.
 */
function guardLeaveOnGuardedSubjects(event: LeaveEvent): void {
    if (!hasGuardedSubjects()) {
        return;
    }

    // The standard cancel pattern: preventDefault per the spec, and the
    // legacy `returnValue` flag the engines require to actually ask.
    event.preventDefault();
    event.returnValue = "";
}

type NavKey = "overview" | "providers" | "models" | "pools" | "routes" | "keys" | "usage";

const NAV_ITEMS: ReadonlyArray<{
    readonly key: NavKey;
    readonly label: string;
    readonly glyph: GlyphName;
    readonly href: string;
}> = [
    {
        key: "overview",
        get label() {
            return translation()(($) => $.navigation.overview);
        },
        glyph: "grid",
        href: hrefs.home(),
    },
    {
        key: "providers",
        get label() {
            return translation()(($) => $.navigation.providers);
        },
        glyph: "plug",
        href: hrefs.providers(),
    },
    {
        key: "models",
        get label() {
            return translation()(($) => $.navigation.inventory);
        },
        glyph: "stack",
        href: hrefs.models(null),
    },
    {
        key: "pools",
        get label() {
            return translation()(($) => $.navigation.pools);
        },
        glyph: "stack",
        href: hrefs.providerPools(),
    },
    {
        key: "routes",
        get label() {
            return translation()(($) => $.navigation.routes);
        },
        glyph: "stack",
        href: hrefs.modelRoutes(),
    },
    {
        key: "keys",
        get label() {
            return translation()(($) => $.navigation.apiKeys);
        },
        glyph: "key",
        href: hrefs.apiKeys(),
    },
    {
        key: "usage",
        get label() {
            return translation()(($) => $.navigation.usage);
        },
        glyph: "activity",
        href: hrefs.usage(),
    },
];

/**
 * The nav tab each route highlights: a route belongs to its section's tab —
 * each creation, detail, and edit screen to its listing's — while the routes
 * no tab owns, today only the not-found fallback, answer `null` and leave
 * every tab unhighlighted. Keyed exhaustively by the route-name union, so a
 * route added to `AppRoute` fails to compile until it declares its tab.
 */
const ROUTE_NAV_KEYS: Record<AppRoute["name"], NavKey | null> = {
    home: "overview",
    bulkPublication: "routes",
    providerPools: "pools",
    providerPoolNew: "pools",
    providerPoolEdit: "pools",
    modelRoutes: "routes",
    modelRouteNew: "routes",
    modelRouteEdit: "routes",
    providers: "providers",
    providerNew: "providers",
    providerDetail: "providers",
    providerEdit: "providers",
    models: "models",
    modelNew: "models",
    modelDetail: "models",
    modelEdit: "models",
    apiKeys: "keys",
    apiKeyNew: "keys",
    apiKeyDetail: "keys",
    apiKeyEdit: "keys",
    usage: "usage",
    usageQuery: "usage",
    notFound: null,
};

function navKeyOfRoute(name: AppRoute["name"]): NavKey | null {
    return ROUTE_NAV_KEYS[name];
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
    // completes. The reset is a signal write, so it runs in the settle phase —
    // a component body is a pure scope and forbids writes — and the keyed
    // boundary settles it once per shell regardless. The shell also always
    // starts at the top — the setup screen can leave the document scrolled
    // when its form was the last thing on a short viewport.
    onSettled(() => {
        resetRefreshStamp();
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
    const publicationRoute = routeNamed(route, "bulkPublication");
    const poolEditRoute = routeNamed(route, "providerPoolEdit");
    const routeEditRoute = routeNamed(route, "modelRouteEdit");
    const routeNewRoute = routeNamed(route, "modelRouteNew");
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
                class="sr-only focus:not-sr-only focus:absolute focus:top-3 focus:start-3 focus:z-20 focus:rounded-lg focus:bg-accent focus:px-3 focus:py-2 focus:text-[13px] focus:font-medium focus:text-accent-ink"
            >
                {translation()(($) => $.shell.skipToContent)}
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
                        <Match
                            when={publicationRoute()}
                            keyed
                        >
                            {(current) => (
                                <BulkPublicationScreen
                                    api={api}
                                    providerFilter={current.providerFilter}
                                />
                            )}
                        </Match>

                        <Match when={route().name === "providerPools"}>
                            <ProviderPoolsScreen api={api} />
                        </Match>
                        <Match when={route().name === "providerPoolNew"}>
                            <ProviderPoolEditorScreen
                                api={api}
                                poolId={null}
                            />
                        </Match>
                        <Match
                            when={poolEditRoute()}
                            keyed
                        >
                            {(current) => (
                                <ProviderPoolEditorScreen
                                    api={api}
                                    poolId={current.poolId}
                                />
                            )}
                        </Match>
                        <Match when={route().name === "modelRoutes"}>
                            <ModelRoutesScreen api={api} />
                        </Match>
                        <Match
                            when={routeNewRoute()}
                            keyed
                        >
                            {(current) => (
                                <ModelRouteEditorScreen
                                    api={api}
                                    routeId={null}
                                    initialModelId={current.initialModelId}
                                />
                            )}
                        </Match>
                        <Match
                            when={routeEditRoute()}
                            keyed
                        >
                            {(current) => (
                                <ModelRouteEditorScreen
                                    api={api}
                                    routeId={current.routeId}
                                    initialModelId={null}
                                />
                            )}
                        </Match>

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
                        {connectedMessage(props.connection.baseUrl)}
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
        get label() {
            return translation()(($) => $.navigation.lightTheme);
        },
    },
    dark: {
        glyph: "moon",
        get label() {
            return translation()(($) => $.navigation.darkTheme);
        },
    },
    system: {
        glyph: "display",
        get label() {
            return translation()(($) => $.navigation.systemTheme);
        },
    },
};

/** The product bar: identity on the left, theme and disconnect on the right. */
function TopBar(): JSX.Element {
    const themeControl = () => THEME_CONTROL_STATES[themePreference()];

    return (
        <header class="sticky top-0 z-10 border-b border-rule bg-canvas/90 backdrop-blur">
            <div class="mx-auto flex min-h-14 w-full max-w-6xl items-center gap-2 px-4 py-2 sm:px-6">
                <AppLink
                    href={hrefs.home()}
                    class="flex items-center gap-2 text-accent"
                >
                    <JanusMark size={19} />
                    <span class="text-[15px] font-semibold tracking-tight text-ink">JanusGate</span>
                </AppLink>

                <div class="ms-auto flex flex-wrap items-center justify-end gap-2">
                    <LanguageControl />
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
 * A not-found address renders links alone and has no read to refresh. Key creation owns the
 * inventory read behind its policy editor, so its screen participates in the refresh pulse.
 */
const REFRESHLESS_ROUTE_NAMES: ReadonlySet<AppRoute["name"]> = new Set([
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
                aria-label={translation()(($) => $.shell.primary)}
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

            <div class="ms-auto flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-[12.5px] text-ink-faint">
                <span
                    class="tnum max-w-56 truncate sm:max-w-72"
                    title={props.connection.baseUrl}
                >
                    <bdi dir="ltr">{props.connection.baseUrl}</bdi>
                </span>
                <span aria-hidden="true">·</span>
                <span>
                    <Show
                        when={refreshStamp()}
                        fallback={translation()(($) => $.remainingText.notRefreshedYet)}
                    >
                        {(stamp) =>
                            translation()(($) => $.domainMessages.updated, {
                                instant: formatRefreshStamp(stamp(), new Date()),
                            })
                        }
                    </Show>
                </span>
                <button
                    type="button"
                    title={
                        props.hasScreenReads
                            ? translation()(
                                  ($) => $.helpers.app.readThisScreensStateFromTheGatewayAgain,
                              )
                            : translation()(
                                  ($) => $.helpers.app.thisScreenReadsNothingFromTheGatewaySo,
                              )
                    }
                    disabled={!props.hasScreenReads}
                    onClick={() => requestRefresh()}
                    class="inline-flex cursor-pointer items-center gap-1 rounded-md px-1.5 py-1 font-medium text-ink-dim transition-colors duration-150 ease-out hover:bg-hover hover:text-ink disabled:cursor-not-allowed disabled:opacity-50"
                >
                    <Glyph name="refresh" />
                    {translation()(($) => $.shell.refresh)}
                </button>
            </div>
        </div>
    );
}

/**
 * Disconnecting is explicit and confirmed; the token must be re-entered. The
 * armed confirm speaks the app's one confirm grammar — the question naming
 * the cost, the action, and the way back — like every in-page confirm, so
 * the bar's one armed control states its consequence before it acts. Focus
 * travels with the confirm: arming moves it onto the question, disarming
 * back to the control — the shared inline-confirm contract
 * (`/src/ui/inline-confirm`).
 */
function DisconnectControl(): JSX.Element {
    const [isArmed, setIsArmed] = createSignal(false);
    const confirmFocus = createInlineConfirmFocus();

    const disarm = (): void => {
        confirmFocus.disarm();
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
                <span
                    class="contents"
                    ref={confirmFocus.restoreDisarmed}
                >
                    <Button
                        tone="ghost"
                        glyph="out"
                        onClick={() => {
                            confirmFocus.arm();
                            setIsArmed(true);
                        }}
                    >
                        {translation()(($) => $.shell.disconnect)}
                    </Button>
                </span>
            }
        >
            <div class="flex flex-wrap items-center gap-2">
                <InlineConfirmQuestion
                    focusArmed={confirmFocus.focusArmed}
                    question={translation()(
                        ($) =>
                            $.helpers.app.disconnectAndForgetTheStoredTokenReconnectingMeansMessage,
                    )}
                />
                <Button
                    tone="danger"
                    onClick={() => {
                        connectionService.forget();
                        goHome();
                    }}
                >
                    {translation()(($) => $.shell.disconnect)}
                </Button>
                <Button
                    tone="ghost"
                    onClick={disarm}
                >
                    {translation()(($) => $.shell.keep)}
                </Button>
            </div>
        </Show>
    );
}
