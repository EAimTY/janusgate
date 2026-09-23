import { type Accessor, createSignal } from "solid-js";
import { isRoutingToken } from "/src/management-api/identifiers";

/**
 * The client-side routes. Each page does one job: the overview reads the
 * gateway's health, the providers pages administer providers, the models
 * pages administer models, the api-keys pages administer serving keys, and
 * the usage pages read request accounting. Detail pages read one thing and
 * link to their edit page; edit pages own every change. The OAuth flows are
 * steps inside the provider creation and reauthorization forms, not
 * destinations of their own.
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
          readonly name: "providerEdit";
          readonly providerId: number;
      }
    | {
          readonly name: "models";
          readonly providerFilter: number | null;
      }
    | {
          readonly name: "modelNew";
          readonly providerFilter: number | null;
          /** A claim-back prefill: the upstream ID the route carried, or `null`. */
          readonly upstreamId: string | null;
      }
    | {
          readonly name: "modelDetail";
          readonly modelId: number;
      }
    | {
          readonly name: "modelEdit";
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
          readonly name: "apiKeyEdit";
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

type AppLocation = {
    readonly pathname: string;
    readonly search: string;
};

/** Builders for every link the app renders, so no href is written twice. */
export const hrefs = {
    home: (): string => "/",
    providers: (): string => "/providers",
    providerNew: (): string => "/providers/new",
    providerDetail: (providerId: number): string => `/providers/${providerId}`,
    providerEdit: (providerId: number): string => `/providers/${providerId}/edit`,
    models: (providerFilter: number | null): string =>
        providerFilter === null ? "/models" : `/models?provider=${providerFilter}`,
    /**
     * `upstreamId` is the claim-back prefill a vanished model's page links
     * with; it rides as the route's second query parameter.
     */
    modelNew: (providerFilter: number | null, upstreamId?: string): string => {
        const parameters = new URLSearchParams();

        if (providerFilter !== null) {
            parameters.set("provider", String(providerFilter));
        }

        if (upstreamId !== undefined) {
            parameters.set("upstream", upstreamId);
        }

        const query = parameters.toString();
        return query === "" ? "/models/new" : `/models/new?${query}`;
    },
    modelDetail: (modelId: number): string => `/models/${modelId}`,
    modelEdit: (modelId: number): string => `/models/${modelId}/edit`,
    apiKeys: (): string => "/api-keys",
    apiKeyNew: (): string => "/api-keys/new",
    apiKeyDetail: (apiKeyId: number): string => `/api-keys/${apiKeyId}`,
    apiKeyEdit: (apiKeyId: number): string => `/api-keys/${apiKeyId}/edit`,
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

    const providerEditMatch = /^\/providers\/([^/]+)\/edit$/.exec(normalizedPath);

    if (providerEditMatch !== null) {
        const providerId = parseRowIdentifier(providerEditMatch[1] ?? null);

        return providerId === null
            ? {
                  name: "notFound",
              }
            : {
                  name: "providerEdit",
                  providerId,
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
        // A prefill must be a value the create form could legally send, so a
        // query value that is not a routing-safe upstream ID reads as absent —
        // the same strictness a provider filter that is not a row identifier
        // gets.
        const upstream = readQueryParameter(search, "upstream");

        return {
            name: "modelNew",
            providerFilter: parseRowIdentifier(readQueryParameter(search, "provider")),
            upstreamId: upstream !== null && isRoutingToken(upstream) ? upstream : null,
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

    const apiKeyEditMatch = /^\/api-keys\/([^/]+)\/edit$/.exec(normalizedPath);

    if (apiKeyEditMatch !== null) {
        const apiKeyId = parseRowIdentifier(apiKeyEditMatch[1] ?? null);

        return apiKeyId === null
            ? {
                  name: "notFound",
              }
            : {
                  name: "apiKeyEdit",
                  apiKeyId,
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

    const modelEditMatch = /^\/models\/([^/]+)\/edit$/.exec(normalizedPath);

    if (modelEditMatch !== null) {
        const modelId = parseRowIdentifier(modelEditMatch[1] ?? null);

        return modelId === null
            ? {
                  name: "notFound",
              }
            : {
                  name: "modelEdit",
                  modelId,
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
