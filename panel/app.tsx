import type { JSX } from "@solidjs/web";
import { createSignal, Match, onSettled, Show, Switch } from "solid-js";
import { PanelApiClient } from "./api-client";
import { AuthOverlay } from "./auth-overlay";
import { type AppRoute, panelConfig } from "./config";
import { ModelsPage } from "./models-page";
import { ProvidersPage } from "./providers-page";
import { buttonPrimaryClasses, errorMessage } from "./ui";

function routeFromLocation(): AppRoute {
    if (window.location.pathname === panelConfig.routes.root) {
        window.history.replaceState(null, "", panelConfig.routes.providers);
        return panelConfig.routes.providers;
    }
    if (window.location.pathname === panelConfig.routes.providers) {
        return panelConfig.routes.providers;
    }
    if (window.location.pathname === panelConfig.routes.models) return panelConfig.routes.models;
    return "not-found";
}

export function App(): JSX.Element {
    const storedToken = localStorage.getItem(panelConfig.tokenStorageKey);
    const [token, setToken] = createSignal<string | null>(storedToken);
    const [tokenInput, setTokenInput] = createSignal(storedToken ?? "");
    const [isAuthenticated, setIsAuthenticated] = createSignal(false);
    const [isCheckingToken, setIsCheckingToken] = createSignal(storedToken !== null);
    const [authError, setAuthError] = createSignal<string | null>(null);
    const [route, setRoute] = createSignal<AppRoute>(routeFromLocation());

    function requireAuthentication(): void {
        localStorage.removeItem(panelConfig.tokenStorageKey);
        setToken(null);
        setTokenInput("");
        setIsAuthenticated(false);
        setIsCheckingToken(false);
        setAuthError("The panel token was rejected or is no longer valid.");
    }

    const client = new PanelApiClient(() => token(), requireAuthentication);

    async function verifyToken(candidateToken: string): Promise<void> {
        if (candidateToken.length === 0) return;
        setIsCheckingToken(true);
        setAuthError(null);
        try {
            await client.probe(candidateToken);
            localStorage.setItem(panelConfig.tokenStorageKey, candidateToken);
            setToken(candidateToken);
            setTokenInput(candidateToken);
            setIsAuthenticated(true);
        } catch (caughtError) {
            localStorage.removeItem(panelConfig.tokenStorageKey);
            setToken(null);
            setIsAuthenticated(false);
            setAuthError(errorMessage(caughtError));
        } finally {
            setIsCheckingToken(false);
        }
    }

    function navigate(
        destination: typeof panelConfig.routes.providers | typeof panelConfig.routes.models,
    ): void {
        if (window.location.pathname !== destination) {
            window.history.pushState(null, "", destination);
        }
        setRoute(destination);
    }

    function handleNavigationClick(
        event: MouseEvent,
        destination: typeof panelConfig.routes.providers | typeof panelConfig.routes.models,
    ): void {
        if (
            event.button !== 0 ||
            event.metaKey ||
            event.ctrlKey ||
            event.shiftKey ||
            event.altKey
        ) {
            return;
        }
        event.preventDefault();
        navigate(destination);
    }

    function forgetToken(): void {
        localStorage.removeItem(panelConfig.tokenStorageKey);
        setToken(null);
        setTokenInput("");
        setIsAuthenticated(false);
        setAuthError(null);
    }

    onSettled(() => {
        const handlePopState = (): void => {
            setRoute(routeFromLocation());
        };
        window.addEventListener("popstate", handlePopState);
        if (storedToken !== null) void verifyToken(storedToken);
        return () => window.removeEventListener("popstate", handlePopState);
    });

    const navigationLinkClasses =
        "flex items-center gap-3 rounded-[11px] border border-transparent px-3.5 py-3 text-sm font-[650] text-[#9ba89f] no-underline transition-colors duration-150 hover:bg-white/4 hover:text-[#eff8f2] focus-visible:outline-3 focus-visible:outline-offset-2 focus-visible:outline-[#29a66f]/25 max-[820px]:gap-1.5 max-[820px]:px-2.5 max-[820px]:py-2";
    const activeNavigationLinkClasses = "border-[#58d797]/14 bg-[#29a66f]/16 text-[#eafff2]";

    return (
        <Show
            when={isAuthenticated()}
            fallback={
                <AuthOverlay
                    token={tokenInput()}
                    error={authError()}
                    isChecking={isCheckingToken()}
                    onTokenInput={(candidateToken) => {
                        setTokenInput(candidateToken);
                    }}
                    onSubmit={() => void verifyToken(tokenInput())}
                />
            }
        >
            <div class="min-h-screen min-w-80 bg-[#f2f5f1] font-sans text-[#18211d] [background:radial-gradient(circle_at_90%_4%,rgb(41_166_111/8%),transparent_26rem),#f2f5f1] [text-rendering:optimizeLegibility] max-[820px]:pt-[66px]">
                <aside class="fixed inset-y-0 left-0 z-20 flex w-62 flex-col border-r border-white/6 px-[18px] py-6 text-[#d9e2dc] [background:linear-gradient(160deg,rgb(41_166_111/9%),transparent_40%),#111915] max-[820px]:inset-x-0 max-[820px]:bottom-auto max-[820px]:h-[66px] max-[820px]:w-auto max-[820px]:flex-row max-[820px]:items-center max-[820px]:px-3.5 max-[820px]:py-2.5">
                    <a
                        class="mx-1.5 mb-[38px] flex items-center gap-3 text-white no-underline focus-visible:outline-3 focus-visible:outline-offset-2 focus-visible:outline-[#29a66f]/25 max-[820px]:mr-auto max-[820px]:mb-0 max-[820px]:ml-0"
                        href={panelConfig.routes.providers}
                        onClick={(event) =>
                            handleNavigationClick(event, panelConfig.routes.providers)
                        }
                    >
                        <span class="grid size-10 shrink-0 place-items-center rounded-xl border border-white/35 bg-linear-to-br from-[#70e2a9] to-[#27a36d] text-[13px] font-[850] tracking-[-0.04em] text-[#0e3323] shadow-[0_12px_30px_rgb(19_113_72/28%)]">
                            JG
                        </span>
                        <span class="grid leading-[1.15] max-[820px]:hidden">
                            <strong class="text-[17px] tracking-[-0.02em]">JanusGate</strong>
                            <small class="mt-1 text-[11px] tracking-[0.12em] text-[#87958c] uppercase">
                                Admin panel
                            </small>
                        </span>
                    </a>
                    <nav
                        class="grid gap-[7px] max-[820px]:flex"
                        aria-label="Primary navigation"
                    >
                        <a
                            class={`${navigationLinkClasses} ${
                                route() === panelConfig.routes.providers
                                    ? activeNavigationLinkClasses
                                    : ""
                            }`}
                            href={panelConfig.routes.providers}
                            aria-current={
                                route() === panelConfig.routes.providers ? "page" : undefined
                            }
                            onClick={(event) =>
                                handleNavigationClick(event, panelConfig.routes.providers)
                            }
                        >
                            <span class="grid size-[25px] place-items-center rounded-[7px] bg-[#29a66f]/11 text-[17px] text-[#58d797] max-[820px]:hidden">
                                ⌁
                            </span>
                            Providers
                        </a>
                        <a
                            class={`${navigationLinkClasses} ${
                                route() === panelConfig.routes.models
                                    ? activeNavigationLinkClasses
                                    : ""
                            }`}
                            href={panelConfig.routes.models}
                            aria-current={
                                route() === panelConfig.routes.models ? "page" : undefined
                            }
                            onClick={(event) =>
                                handleNavigationClick(event, panelConfig.routes.models)
                            }
                        >
                            <span class="grid size-[25px] place-items-center rounded-[7px] bg-[#29a66f]/11 text-[17px] text-[#58d797] max-[820px]:hidden">
                                ◇
                            </span>
                            Models
                        </a>
                    </nav>
                    <div class="mt-auto grid gap-3 border-t border-white/8 px-2.5 pt-[17px] pb-1 max-[820px]:hidden">
                        <div class="flex items-center gap-2 text-xs text-[#a8b4ac]">
                            <span class="size-[7px] rounded-full bg-[#58d797] shadow-[0_0_0_4px_rgb(88_215_151/10%)]" />
                            Authenticated
                        </div>
                        <button
                            class="w-fit border-0 bg-transparent p-0 text-xs text-[#748179] underline underline-offset-3 hover:text-[#d2ddd6] focus-visible:outline-3 focus-visible:outline-offset-2 focus-visible:outline-[#29a66f]/25"
                            type="button"
                            onClick={forgetToken}
                        >
                            Forget token
                        </button>
                    </div>
                </aside>
                <main class="min-w-0 ml-62 max-[820px]:ml-0">
                    <Switch>
                        <Match when={route() === panelConfig.routes.providers}>
                            <ProvidersPage client={client} />
                        </Match>
                        <Match when={route() === panelConfig.routes.models}>
                            <ModelsPage client={client} />
                        </Match>
                        <Match when={route() === "not-found"}>
                            <section class="grid min-h-screen place-content-center justify-items-start p-10">
                                <p class="m-0 text-[11px] font-extrabold tracking-[0.16em] text-[#19784e] uppercase">
                                    404
                                </p>
                                <h1 class="mt-[3px] mb-2 text-[clamp(32px,4vw,46px)] leading-none font-[760] tracking-[-0.045em] text-[#18211d]">
                                    Panel route not found
                                </h1>
                                <p class="m-0 max-w-[670px] text-sm text-[#66726b]">
                                    The Worker served the SPA fallback, but this client route does
                                    not exist.
                                </p>
                                <button
                                    class={`${buttonPrimaryClasses} mt-[22px]`}
                                    type="button"
                                    onClick={() => navigate(panelConfig.routes.providers)}
                                >
                                    Go to providers
                                </button>
                            </section>
                        </Match>
                    </Switch>
                </main>
            </div>
        </Show>
    );
}
