import { type Accessor, createSignal } from "solid-js";

/**
 * The client-side routes. Each page does one job: the overview reads the
 * gateway's health, the providers pages administer providers, the models
 * pages administer models, the api-keys pages administer serving keys, and
 * the usage pages read request accounting. The OAuth flows are steps inside
 * the provider creation and reauthorization forms, not destinations of their
 * own.
 */
export type AppRoute =
    | {
          readonly name: "home";
      }
    | {
          readonly name: "providers";
      }
    | {
          readonly name: "providerNew";
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
          readonly name: "modelNew";
          readonly providerFilter: number | null;
      }
    | {
          readonly name: "modelDetail";
          readonly modelId: number;
      }
    | {
          readonly name: "apiKeys";
      }
    | {
          readonly name: "apiKeyNew";
      }
    | {
          readonly name: "apiKeyDetail";
          readonly apiKeyId: number;
      }
    | {
          readonly name: "usage";
      }
    | {
          readonly name: "usageQuery";
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
    home: (): string => "/",
    providers: (): string => "/providers",
    providerNew: (): string => "/providers/new",
    providerDetail: (providerId: number): string => `/providers/${providerId}`,
    models: (providerFilter: number | null): string =>
        providerFilter === null ? "/models" : `/models?provider=${providerFilter}`,
    modelNew: (providerFilter: number | null): string =>
        providerFilter === null ? "/models/new" : `/models/new?provider=${providerFilter}`,
    modelDetail: (modelId: number): string => `/models/${modelId}`,
    apiKeys: (): string => "/api-keys",
    apiKeyNew: (): string => "/api-keys/new",
    apiKeyDetail: (apiKeyId: number): string => `/api-keys/${apiKeyId}`,
    usage: (): string => "/usage",
    usageQuery: (): string => "/usage/query",
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

    if (normalizedPath === "/") {
        return {
            name: "home",
        };
    }

    if (normalizedPath === "/providers/new") {
        return {
            name: "providerNew",
        };
    }

    if (normalizedPath === "/providers") {
        return {
            name: "providers",
        };
    }

    const providerMatch = /^\/providers\/([^/]+)$/.exec(normalizedPath);

    if (providerMatch !== null) {
        const providerId = parseRowIdentifier(providerMatch[1] ?? null);

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

    if (normalizedPath === "/models/new") {
        return {
            name: "modelNew",
            providerFilter: parseRowIdentifier(readQueryParameter(search, "provider")),
        };
    }

    if (normalizedPath === "/api-keys") {
        return {
            name: "apiKeys",
        };
    }

    if (normalizedPath === "/api-keys/new") {
        return {
            name: "apiKeyNew",
        };
    }

    const apiKeyMatch = /^\/api-keys\/([^/]+)$/.exec(normalizedPath);

    if (apiKeyMatch !== null) {
        const apiKeyId = parseRowIdentifier(apiKeyMatch[1] ?? null);

        return apiKeyId === null
            ? {
                  name: "notFound",
              }
            : {
                  name: "apiKeyDetail",
                  apiKeyId,
              };
    }

    if (normalizedPath === "/usage") {
        return {
            name: "usage",
        };
    }

    if (normalizedPath === "/usage/query") {
        return {
            name: "usageQuery",
        };
    }

    const modelMatch = /^\/models\/([^/]+)$/.exec(normalizedPath);

    if (modelMatch !== null) {
        const modelId = parseRowIdentifier(modelMatch[1] ?? null);

        return modelId === null
            ? {
                  name: "notFound",
              }
            : {
                  name: "modelDetail",
                  modelId,
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
