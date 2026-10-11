import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { EMPTY_USAGE_FILTER_DRAFT, type UsageFilterDraft } from "./features/usage/filter";
import { type AppRoute, hrefs, matchAppRoute, routeTitle } from "./router";

/**
 * The client-side routes: one address family per entity (detail and edit
 * sharing one pattern), trailing slashes addressing the same resource,
 * malformed row segments leading to not-found, strict query reading (an
 * unaddressable filter or prefill reading as absent), and the builders every
 * link renders from.
 */

beforeEach(() => {
    window.history.replaceState(null, "", "/");
});

afterEach(() => {
    window.history.replaceState(null, "", "/");
});

describe("matchAppRoute", () => {
    test("answers the fixed listing and creation addresses", () => {
        expect(matchAppRoute("/", "")).toEqual({
            name: "home",
        });
        expect(matchAppRoute("/providers", "")).toEqual({
            name: "providers",
        });
        expect(matchAppRoute("/providers/new", "")).toEqual({
            name: "providerNew",
        });
        expect(matchAppRoute("/models", "")).toEqual({
            name: "models",
            providerFilter: null,
        });
        expect(matchAppRoute("/pools", "")).toEqual({
            name: "providerPools",
        });
        expect(matchAppRoute("/pools/new", "")).toEqual({
            name: "providerPoolNew",
        });
        expect(matchAppRoute("/routes", "")).toEqual({
            name: "modelRoutes",
        });
        expect(matchAppRoute("/routes/new", "")).toEqual({
            name: "modelRouteNew",
            initialModelId: null,
        });
        expect(matchAppRoute("/routes/publish", "")).toEqual({
            name: "bulkPublication",
            providerFilter: null,
        });
        expect(matchAppRoute("/api-keys", "")).toEqual({
            name: "apiKeys",
        });
        expect(matchAppRoute("/api-keys/new", "")).toEqual({
            name: "apiKeyNew",
        });
        expect(matchAppRoute("/usage", "")).toEqual({
            name: "usage",
            filter: EMPTY_USAGE_FILTER_DRAFT,
        });
        expect(matchAppRoute("/usage/query", "")).toEqual({
            name: "usageQuery",
            subject: null,
        });
    });

    test("matches each entity's detail and edit addresses through one pattern", () => {
        expect(matchAppRoute("/providers/7", "")).toEqual({
            name: "providerDetail",
            providerId: 7,
        });
        expect(matchAppRoute("/providers/7/edit", "")).toEqual({
            name: "providerEdit",
            providerId: 7,
        });
        expect(matchAppRoute("/models/3", "")).toEqual({
            name: "modelDetail",
            modelId: 3,
        });
        expect(matchAppRoute("/models/3/edit", "")).toEqual({
            name: "modelEdit",
            modelId: 3,
        });
        expect(matchAppRoute("/api-keys/12", "")).toEqual({
            name: "apiKeyDetail",
            apiKeyId: 12,
        });
        expect(matchAppRoute("/api-keys/12/edit", "")).toEqual({
            name: "apiKeyEdit",
            apiKeyId: 12,
        });
    });

    test("resolves both editor spellings for pools and public routes", () => {
        // The shared editor serves detail and edit alike.
        expect(matchAppRoute("/pools/5", "")).toEqual({
            name: "providerPoolEdit",
            poolId: 5,
        });
        expect(matchAppRoute("/pools/5/edit", "")).toEqual({
            name: "providerPoolEdit",
            poolId: 5,
        });
        expect(matchAppRoute("/routes/9", "")).toEqual({
            name: "modelRouteEdit",
            routeId: 9,
        });
        expect(matchAppRoute("/routes/9/edit", "")).toEqual({
            name: "modelRouteEdit",
            routeId: 9,
        });
    });

    test("normalizes trailing slashes to the same resource", () => {
        expect(matchAppRoute("/providers/", "")).toEqual({
            name: "providers",
        });
        expect(matchAppRoute("/providers/7/", "")).toEqual({
            name: "providerDetail",
            providerId: 7,
        });
        expect(matchAppRoute("/models/new/", "")).toEqual({
            name: "modelNew",
            providerFilter: null,
            upstreamId: null,
        });
    });

    test("leads a malformed row segment to not-found, never to a partial match", () => {
        expect(matchAppRoute("/providers/abc", "")).toEqual({
            name: "notFound",
        });
        expect(matchAppRoute("/providers/1e3", "")).toEqual({
            name: "notFound",
        });
        expect(matchAppRoute("/pools/xyz/edit", "")).toEqual({
            name: "notFound",
        });
    });

    test("puts the bulk-publication address before the public-route family", () => {
        // `/routes/publish` is the publication page, never route 404.
        expect(matchAppRoute("/routes/publish", "")).toEqual({
            name: "bulkPublication",
            providerFilter: null,
        });
    });

    test("answers not-found for everything outside the families", () => {
        expect(matchAppRoute("/nowhere", "")).toEqual({
            name: "notFound",
        });
        expect(matchAppRoute("/providers/7/delete", "")).toEqual({
            name: "notFound",
        });
    });

    test("reads the filters and prefills the query strings carry, strictly", () => {
        expect(matchAppRoute("/models", "?provider=4")).toEqual({
            name: "models",
            providerFilter: 4,
        });
        expect(matchAppRoute("/models", "?provider=abc")).toEqual({
            name: "models",
            providerFilter: null,
        });
        expect(matchAppRoute("/routes/publish", "?provider=2")).toEqual({
            name: "bulkPublication",
            providerFilter: 2,
        });
        expect(matchAppRoute("/routes/new", "?model=8")).toEqual({
            name: "modelRouteNew",
            initialModelId: 8,
        });
        expect(matchAppRoute("/routes/new", "?model=oops")).toEqual({
            name: "modelRouteNew",
            initialModelId: null,
        });
    });

    test("reads a model-creation upstream prefill only when it is a routing-safe token", () => {
        expect(matchAppRoute("/models/new", "?provider=2&upstream=gpt-4o")).toEqual({
            name: "modelNew",
            providerFilter: 2,
            upstreamId: "gpt-4o",
        });
        // A claim the create form could not legally send reads as absent.
        expect(matchAppRoute("/models/new", "?upstream=models/gpt-x")).toEqual({
            name: "modelNew",
            providerFilter: null,
            upstreamId: null,
        });
    });

    test("carries the usage stream's filter draft and the aggregates subject", () => {
        const route = matchAppRoute(
            "/usage",
            "?window=7d&provider=3&outcome=upstream_error,internal",
        );

        expect(route).toEqual({
            name: "usage",
            filter: {
                ...EMPTY_USAGE_FILTER_DRAFT,
                window: "7d",
                providerId: "3",
                outcome: "failures",
            },
        });

        expect(matchAppRoute("/usage/query", "?model=11")).toEqual({
            name: "usageQuery",
            subject: {
                dimension: "model",
                id: 11,
            },
        });
        // None named, several named, or a non-row value: the bare page.
        expect(matchAppRoute("/usage/query", "?model=11&provider=3")).toEqual({
            name: "usageQuery",
            subject: null,
        });
        expect(matchAppRoute("/usage/query", "?model=abc")).toEqual({
            name: "usageQuery",
            subject: null,
        });
    });
});

describe("hrefs", () => {
    test("builds every link the app renders, with absent filters spelling bare addresses", () => {
        expect(hrefs.home()).toBe("/");
        expect(hrefs.providers()).toBe("/providers");
        expect(hrefs.providerNew()).toBe("/providers/new");
        expect(hrefs.providerDetail(7)).toBe("/providers/7");
        expect(hrefs.providerEdit(7)).toBe("/providers/7/edit");
        expect(hrefs.models(null)).toBe("/models");
        expect(hrefs.models(4)).toBe("/models?provider=4");
        expect(hrefs.modelNew(null)).toBe("/models/new");
        expect(hrefs.modelNew(4)).toBe("/models/new?provider=4");
        expect(hrefs.modelNew(4, "gpt-4o")).toBe("/models/new?provider=4&upstream=gpt-4o");
        expect(hrefs.modelNew(null, "gpt-4o")).toBe("/models/new?upstream=gpt-4o");
        expect(hrefs.modelDetail(3)).toBe("/models/3");
        expect(hrefs.modelEdit(3)).toBe("/models/3/edit");
        expect(hrefs.providerPools()).toBe("/pools");
        expect(hrefs.providerPoolNew()).toBe("/pools/new");
        expect(hrefs.providerPoolEdit(5)).toBe("/pools/5/edit");
        expect(hrefs.modelRoutes()).toBe("/routes");
        expect(hrefs.modelRouteNew()).toBe("/routes/new");
        expect(hrefs.modelRouteNew(8)).toBe("/routes/new?model=8");
        expect(hrefs.modelRouteEdit(9)).toBe("/routes/9/edit");
        expect(hrefs.bulkPublication(null)).toBe("/routes/publish");
        expect(hrefs.bulkPublication(2)).toBe("/routes/publish?provider=2");
        expect(hrefs.apiKeys()).toBe("/api-keys");
        expect(hrefs.apiKeyNew()).toBe("/api-keys/new");
        expect(hrefs.apiKeyDetail(12)).toBe("/api-keys/12");
        expect(hrefs.apiKeyEdit(12)).toBe("/api-keys/12/edit");
        expect(hrefs.usage()).toBe("/usage");
        expect(hrefs.usageQuery()).toBe("/usage/query");
        expect(
            hrefs.usageQuery({
                dimension: "api_key",
                id: 8,
            }),
        ).toBe("/usage/query?key=8");
    });

    test("carries the usage stream's draft as its query, the unfiltered draft spelling bare", () => {
        const draft: UsageFilterDraft = {
            ...EMPTY_USAGE_FILTER_DRAFT,
            window: "24h",
            modelRouteId: "5",
        };

        expect(hrefs.usage(draft)).toBe("/usage?window=24h&route=5");
        expect(matchAppRoute("/usage", "?window=24h&route=5")).toEqual({
            name: "usage",
            filter: draft,
        });
    });
});

describe("routeTitle", () => {
    test("names the screen over the product, the addressed row by its identifier", () => {
        const titled = (route: AppRoute): string => routeTitle(route);

        expect(
            titled({
                name: "home",
            }),
        ).toBe("Overview · JanusGate");
        expect(
            titled({
                name: "providerEdit",
                providerId: 7,
            }),
        ).toBe("Edit provider #7 · JanusGate");
        expect(
            titled({
                name: "notFound",
            }),
        ).toBe("Page not found · JanusGate");
        expect(
            titled({
                name: "usageQuery",
                subject: null,
            }),
        ).toBe("Usage aggregates · JanusGate");
    });
});
