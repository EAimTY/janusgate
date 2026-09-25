import { type Accessor, createSignal } from "solid-js";
import type { UsageGroupBy } from "/src/management-api/generated/v0";
import { isRoutingToken } from "/src/management-api/identifiers";
import { parseRowIdentifier } from "./row-id";
import {
    EMPTY_USAGE_FILTER_DRAFT,
    parseUsageFilterDraft,
    serializeUsageFilterDraft,
    type UsageFilterDraft,
} from "./usage-filter";

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
          /** The filter the route carries; the default draft is the unfiltered stream. */
          readonly filter: UsageFilterDraft;
      }
    | {
          readonly name: "usageQuery";
          /**
           * The subject the address carried — one entity dimension and the
           * row the aggregates page preselects — or `null` for the bare page.
           */
          readonly subject: UsageQuerySubject | null;
      }
    | {
          readonly name: "notFound";
      };

type AppLocation = {
    readonly pathname: string;
    readonly search: string;
};

/** The entity dimensions an aggregates link can carry its subject as: the ones with a filter field of their own. */
export type UsageQuerySubjectDimension = Extract<UsageGroupBy, "provider" | "model" | "api_key">;

/** The subject an aggregates link carries: one entity dimension and the row to preselect there. */
export type UsageQuerySubject = {
    readonly dimension: UsageQuerySubjectDimension;
    readonly id: number;
};

/** The query parameter one subject dimension rides under — the usage stream's own names, so both usage routes address an entity alike. */
const USAGE_QUERY_SUBJECT_PARAMETERS: Record<UsageQuerySubjectDimension, string> = {
    provider: "provider",
    model: "model",
    api_key: "key",
};

const USAGE_QUERY_SUBJECT_DIMENSIONS: ReadonlyArray<UsageQuerySubjectDimension> = [
    "provider",
    "model",
    "api_key",
];

/**
 * The query string one aggregates link carries for its subject: a single
 * `parameter=row-id` pair, readable by the same strictness the usage
 * stream's route applies to its own entity fields.
 */
function serializeUsageQuerySubject(subject: UsageQuerySubject): string {
    return `${USAGE_QUERY_SUBJECT_PARAMETERS[subject.dimension]}=${subject.id}`;
}

/**
 * The subject one aggregates address carries: exactly one of the stream's
 * entity parameters may name a row. Any other reading — none named, several
 * named, a value that is not a row identifier — is the bare page, exactly as
 * the usage stream reads an unaddressable filter as absent.
 */
function parseUsageQuerySubject(search: string): UsageQuerySubject | null {
    const parameters = new URLSearchParams(search);
    const named: UsageQuerySubject[] = [];

    for (const dimension of USAGE_QUERY_SUBJECT_DIMENSIONS) {
        const id = parseRowIdentifier(parameters.get(USAGE_QUERY_SUBJECT_PARAMETERS[dimension]));

        if (id !== null) {
            named.push({
                dimension,
                id,
            });
        }
    }

    const [subject] = named;
    return named.length === 1 ? (subject ?? null) : null;
}

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
    /**
     * The listing's filter as its route carries it: absent means unfiltered.
     */
    usage: (filter: UsageFilterDraft = EMPTY_USAGE_FILTER_DRAFT): string => {
        const query = serializeUsageFilterDraft(filter);

        return query === "" ? "/usage" : `/usage?${query}`;
    },
    /**
     * The aggregates page; the optional subject — one entity dimension and
     * the row to preselect there — rides under the usage stream's own
     * parameter names, so both usage routes address an entity alike.
     */
    usageQuery: (subject?: UsageQuerySubject): string =>
        subject === undefined
            ? "/usage/query"
            : `/usage/query?${serializeUsageQuerySubject(subject)}`,
} as const;

/**
 * The screen's own name for the document title, worded as the screens' own
 * headings are, with the row a route addresses named by its identifier — the
 * shell holds only what the address carries, and a row's label is the
 * screen's read to resolve.
 */
function routeScreenName(route: AppRoute): string {
    switch (route.name) {
        case "home":
            return "Overview";
        case "providers":
            return "Providers";
        case "providerNew":
            return "Create provider";
        case "providerDetail":
            return `Provider #${route.providerId}`;
        case "providerEdit":
            return `Edit provider #${route.providerId}`;
        case "models":
            return "Models";
        case "modelNew":
            return "Create model";
        case "modelDetail":
            return `Model #${route.modelId}`;
        case "modelEdit":
            return `Edit model #${route.modelId}`;
        case "apiKeys":
            return "API keys";
        case "apiKeyNew":
            return "Create API key";
        case "apiKeyDetail":
            return `API key #${route.apiKeyId}`;
        case "apiKeyEdit":
            return `Edit API key #${route.apiKeyId}`;
        case "usage":
            return "Usage";
        case "usageQuery":
            return "Usage aggregates";
        case "notFound":
            return "Page not found";
    }
}

/**
 * The document title one route presents: the screen's name over the product,
 * so the browser's tab, history entries, bookmarks, and screen-reader page
 * announcements say which screen of the console they hold.
 */
export function routeTitle(route: AppRoute): string {
    return `${routeScreenName(route)} · JanusGate`;
}

function readQueryParameter(search: string, name: string): string | null {
    return new URLSearchParams(search).get(name);
}

/**
 * One pattern per entity address family: `<prefix>/<row id>` with the
 * optional `/edit` suffix captured as group 2, so one match serves an
 * entity's detail and edit pages alike. Group 1 carries the row segment.
 */
const PROVIDER_ADDRESS = /^\/providers\/([^/]+)(\/edit)?$/;
const MODEL_ADDRESS = /^\/models\/([^/]+)(\/edit)?$/;
const API_KEY_ADDRESS = /^\/api-keys\/([^/]+)(\/edit)?$/;

/**
 * Matches one entity's two addresses — `<prefix>/<row id>` for detail and
 * `<prefix>/<row id>/edit` for edit. The caller's builders construct the
 * route from the parsed row identifier, so the AppRoute construction stays
 * at the call; an address whose segment is not a row identifier is
 * not-found, and a path outside the family answers `null` so the next route
 * gets a turn.
 */
function matchEntityRoute(
    pathname: string,
    pattern: RegExp,
    detail: (rowId: number) => AppRoute,
    edit: (rowId: number) => AppRoute,
): AppRoute | null {
    const match = pattern.exec(pathname);

    if (match === null) {
        return null;
    }

    const rowId = parseRowIdentifier(match[1] ?? null);

    if (rowId === null) {
        return {
            name: "notFound",
        };
    }

    return match[2] === undefined ? detail(rowId) : edit(rowId);
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

    const providerRoute = matchEntityRoute(
        normalizedPath,
        PROVIDER_ADDRESS,
        (providerId) => ({
            name: "providerDetail",
            providerId,
        }),
        (providerId) => ({
            name: "providerEdit",
            providerId,
        }),
    );

    if (providerRoute !== null) {
        return providerRoute;
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

    const apiKeyRoute = matchEntityRoute(
        normalizedPath,
        API_KEY_ADDRESS,
        (apiKeyId) => ({
            name: "apiKeyDetail",
            apiKeyId,
        }),
        (apiKeyId) => ({
            name: "apiKeyEdit",
            apiKeyId,
        }),
    );

    if (apiKeyRoute !== null) {
        return apiKeyRoute;
    }

    if (normalizedPath === "/usage") {
        return {
            name: "usage",
            filter: parseUsageFilterDraft(search),
        };
    }

    if (normalizedPath === "/usage/query") {
        return {
            name: "usageQuery",
            subject: parseUsageQuerySubject(search),
        };
    }

    const modelRoute = matchEntityRoute(
        normalizedPath,
        MODEL_ADDRESS,
        (modelId) => ({
            name: "modelDetail",
            modelId,
        }),
        (modelId) => ({
            name: "modelEdit",
            modelId,
        }),
    );

    if (modelRoute !== null) {
        return modelRoute;
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
    const next = readLocation();
    const previous = location();
    const isSameAddress = previous.pathname === next.pathname && previous.search === next.search;

    // A traversal that carries the address already held — a hash-only move
    // among them, which readLocation does not see — is not a navigation:
    // writing it would notify the signal with a fresh, equal object and
    // remount every keyed screen.
    if (isSameAddress) {
        return;
    }

    setLocation(next);
});
