import { type Accessor, createSignal } from "solid-js";

/**
 * The client-side routes of the app (README §6). The host serves `index.html`
 * for unknown paths, so every route here is also a deep link; anything the
 * matcher does not recognize renders the not-found screen.
 */
export type AppRoute =
    | {
          readonly name: "providers";
      }
    | {
          readonly name: "providerCreate";
      }
    | {
          readonly name: "providerDetail";
          readonly providerId: number;
      }
    | {
          readonly name: "models";
          readonly providerFilter: number | null;
      }
    | {
          readonly name: "oauthStart";
          readonly reauthorizeProviderId: number | null;
      }
    | {
          readonly name: "notFound";
      };

export type AppLocation = {
    readonly pathname: string;
    readonly search: string;
};

/** Builders for every link the app renders, so no href is written twice. */
export const hrefs = {
    providers: (): string => "/providers",
    providerCreate: (): string => "/providers/new",
    providerDetail: (providerId: number): string => `/providers/${providerId}`,
    models: (providerFilter: number | null): string =>
        providerFilter === null ? "/models" : `/models?provider=${providerFilter}`,
    oauthStart: (reauthorizeProviderId: number | null): string =>
        reauthorizeProviderId === null
            ? "/oauth/start"
            : `/oauth/start?provider=${reauthorizeProviderId}`,
} as const;

/**
 * Reads a row identifier out of a path segment or query value. Only a plain
 * decimal integer addresses a row; anything else reads as absent.
 */
export function parseRowIdentifier(value: string | null): number | null {
    if (value === null || !/^\d+$/.test(value)) {
        return null;
    }
    const identifier = Number(value);
    return Number.isSafeInteger(identifier) ? identifier : null;
}

function readQueryParameter(search: string, name: string): string | null {
    return new URLSearchParams(search).get(name);
}

export function matchAppRoute(pathname: string, search: string): AppRoute {
    // A trailing slash addresses the same resource.
    const normalizedPath = pathname !== "/" ? pathname.replace(/\/+$/, "") : pathname;

    if (normalizedPath === "/" || normalizedPath === "/providers") {
        return {
            name: "providers",
        };
    }
    if (normalizedPath === "/providers/new") {
        return {
            name: "providerCreate",
        };
    }

    const providerDetailMatch = /^\/providers\/([^/]+)$/.exec(normalizedPath);
    if (providerDetailMatch !== null) {
        const providerId = parseRowIdentifier(providerDetailMatch[1] ?? null);
        return providerId === null
            ? {
                  name: "notFound",
              }
            : {
                  name: "providerDetail",
                  providerId,
              };
    }

    if (normalizedPath === "/models") {
        return {
            name: "models",
            providerFilter: parseRowIdentifier(readQueryParameter(search, "provider")),
        };
    }

    if (normalizedPath === "/oauth/start") {
        return {
            name: "oauthStart",
            reauthorizeProviderId: parseRowIdentifier(readQueryParameter(search, "provider")),
        };
    }

    return {
        name: "notFound",
    };
}

function readLocation(): AppLocation {
    return {
        pathname: window.location.pathname,
        search: window.location.search,
    };
}

const [location, setLocation] = createSignal<AppLocation>(readLocation());

/** The current location; route matching memoizes over it. */
export const currentLocation: Accessor<AppLocation> = location;

/** Client-side navigation: push a history entry and re-render. */
export function navigate(target: string): void {
    // Selecting the route already shown is not a navigation: it would only add
    // a duplicate history entry and remount the screen.
    if (target === `${window.location.pathname}${window.location.search}`) {
        return;
    }
    window.history.pushState(null, "", target);
    setLocation(readLocation());
    window.scrollTo(0, 0);
}

window.addEventListener("popstate", () => {
    setLocation(readLocation());
});
