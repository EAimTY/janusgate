import { describe, expect, it } from "vitest";
import { drainWrites } from "/src/screens/screen-test-support";
import { currentLocation, hrefs, isCurrentAddress, matchAppRoute, routeTitle } from "./router";
import { EMPTY_USAGE_FILTER_DRAFT } from "./usage-filter";

function match(target: string) {
    const [pathname = "/", search = ""] = target
        .split("?")
        .map((part, index) => (index === 1 ? `?${part}` : part));

    return matchAppRoute(pathname, search);
}

describe("matchAppRoute", () => {
    it("addresses the overview at the root", () => {
        expect(match("/")).toEqual({
            name: "home",
        });
    });

    it("addresses the provider list", () => {
        expect(match("/providers")).toEqual({
            name: "providers",
        });
        expect(match("/providers/")).toEqual({
            name: "providers",
        });
    });

    it("addresses provider creation before the identifier route", () => {
        expect(match("/providers/new")).toEqual({
            name: "providerNew",
        });
        expect(match("/providers/new/")).toEqual({
            name: "providerNew",
        });
    });

    it("addresses one provider by its identifier, and its edit page", () => {
        expect(match("/providers/12")).toEqual({
            name: "providerDetail",
            providerId: 12,
        });
        expect(match("/providers/12/")).toEqual({
            name: "providerDetail",
            providerId: 12,
        });
        expect(match("/providers/12/edit")).toEqual({
            name: "providerEdit",
            providerId: 12,
        });
        expect(match("/providers/12/edit/")).toEqual({
            name: "providerEdit",
            providerId: 12,
        });
    });

    it("reads a provider segment that is not a row identifier as not found", () => {
        expect(match("/providers/codex")).toEqual({
            name: "notFound",
        });
        expect(match("/providers/codex/edit")).toEqual({
            name: "notFound",
        });
        expect(match("/providers/12/extra")).toEqual({
            name: "notFound",
        });
    });

    it("addresses the models overview, optionally filtered to one provider", () => {
        expect(match("/models")).toEqual({
            name: "models",
            providerFilter: null,
        });
        expect(match("/models?provider=4")).toEqual({
            name: "models",
            providerFilter: 4,
        });
    });

    it("reads a provider filter that is not a row identifier as absent", () => {
        expect(match("/models?provider=codex")).toEqual({
            name: "models",
            providerFilter: null,
        });
    });

    it("addresses model creation before the identifier route", () => {
        expect(match("/models/new")).toEqual({
            name: "modelNew",
            providerFilter: null,
            upstreamId: null,
        });
        expect(match("/models/new?provider=4")).toEqual({
            name: "modelNew",
            providerFilter: 4,
            upstreamId: null,
        });
        expect(match("/models/new?provider=codex")).toEqual({
            name: "modelNew",
            providerFilter: null,
            upstreamId: null,
        });
    });

    it("carries a claim-back upstream ID into model creation, and no other value", () => {
        expect(match("/models/new?provider=4&upstream=gpt-x")).toEqual({
            name: "modelNew",
            providerFilter: 4,
            upstreamId: "gpt-x",
        });
        expect(match("/models/new?upstream=gpt-x")).toEqual({
            name: "modelNew",
            providerFilter: null,
            upstreamId: "gpt-x",
        });
        // Only a routing-safe upstream ID addresses the prefill; anything
        // else reads as absent, like a provider filter that is not a row
        // identifier.
        expect(match("/models/new?provider=4&upstream=gpt%20x")).toEqual({
            name: "modelNew",
            providerFilter: 4,
            upstreamId: null,
        });
        expect(match("/models/new?provider=4&upstream=")).toEqual({
            name: "modelNew",
            providerFilter: 4,
            upstreamId: null,
        });
    });

    it("addresses the api-keys list, creation, detail, and edit", () => {
        expect(match("/api-keys")).toEqual({
            name: "apiKeys",
        });
        expect(match("/api-keys/new")).toEqual({
            name: "apiKeyNew",
        });
        expect(match("/api-keys/7")).toEqual({
            name: "apiKeyDetail",
            apiKeyId: 7,
        });
        expect(match("/api-keys/7/edit")).toEqual({
            name: "apiKeyEdit",
            apiKeyId: 7,
        });
        expect(match("/api-keys/codex")).toEqual({
            name: "notFound",
        });
        expect(match("/api-keys/codex/edit")).toEqual({
            name: "notFound",
        });
    });

    it("addresses one model by its row identifier, and its edit page", () => {
        expect(match("/models/9")).toEqual({
            name: "modelDetail",
            modelId: 9,
        });
        expect(match("/models/9/")).toEqual({
            name: "modelDetail",
            modelId: 9,
        });
        expect(match("/models/9/edit")).toEqual({
            name: "modelEdit",
            modelId: 9,
        });
        expect(match("/models/9/edit/")).toEqual({
            name: "modelEdit",
            modelId: 9,
        });
    });

    it("reads a model address that is not a row identifier as not found", () => {
        expect(match("/models/codex")).toEqual({
            name: "notFound",
        });
        // The previous (provider, upstream) pair addresses no longer resolve.
        expect(match("/models/4/gpt-4o")).toEqual({
            name: "notFound",
        });
        expect(match("/models/4/gpt%204o%20mini")).toEqual({
            name: "notFound",
        });
    });

    it("addresses the usage requests and their aggregation", () => {
        expect(match("/usage")).toEqual({
            name: "usage",
            filter: EMPTY_USAGE_FILTER_DRAFT,
        });
        expect(match("/usage/")).toEqual({
            name: "usage",
            filter: EMPTY_USAGE_FILTER_DRAFT,
        });
        expect(match("/usage/query")).toEqual({
            name: "usageQuery",
            subject: null,
        });
        expect(match("/usage/query/")).toEqual({
            name: "usageQuery",
            subject: null,
        });
        expect(match("/usage/other")).toEqual({
            name: "notFound",
        });
    });

    it("carries the listing's filter through its route", () => {
        const filter = {
            window: "24h",
            providerId: "3",
            modelId: "",
            apiKeyId: "",
            endpointType: "",
            outcome: "failures",
            streamChoice: "",
            statusInput: "",
        } as const;

        expect(match(hrefs.usage(filter))).toEqual({
            name: "usage",
            filter,
        });
    });

    it("carries the aggregates link's subject through its route", () => {
        expect(match("/usage/query?provider=7")).toEqual({
            name: "usageQuery",
            subject: {
                dimension: "provider",
                id: 7,
            },
        });
        expect(match("/usage/query?model=9")).toEqual({
            name: "usageQuery",
            subject: {
                dimension: "model",
                id: 9,
            },
        });
        expect(match("/usage/query?key=3")).toEqual({
            name: "usageQuery",
            subject: {
                dimension: "api_key",
                id: 3,
            },
        });
    });

    it("reads anything but exactly one entity parameter as no subject", () => {
        // The bare page.
        expect(match("/usage/query")).toEqual({
            name: "usageQuery",
            subject: null,
        });

        // A value that is not a row identifier names no row, and neither does
        // a digit string beyond the safe-integer range.
        expect(match("/usage/query?provider=codex")).toEqual({
            name: "usageQuery",
            subject: null,
        });
        expect(match("/usage/query?provider=")).toEqual({
            name: "usageQuery",
            subject: null,
        });
        expect(match("/usage/query?provider=99999999999999999999")).toEqual({
            name: "usageQuery",
            subject: null,
        });

        // The aggregates link carries one subject, so an address naming
        // several entities reads as naming none.
        expect(match("/usage/query?provider=7&model=9")).toEqual({
            name: "usageQuery",
            subject: null,
        });
    });

    it("reads unknown paths as not found", () => {
        expect(match("/panel")).toEqual({
            name: "notFound",
        });
        expect(match("/sections")).toEqual({
            name: "notFound",
        });
        expect(match("/oauth")).toEqual({
            name: "notFound",
        });
        expect(match("/tracks/1")).toEqual({
            name: "notFound",
        });
    });

    it("keeps every hrefs builder and the matcher in agreement", () => {
        expect(match(hrefs.home())).toEqual({
            name: "home",
        });
        expect(match(hrefs.providers())).toEqual({
            name: "providers",
        });
        expect(match(hrefs.providerNew())).toEqual({
            name: "providerNew",
        });
        expect(match(hrefs.providerDetail(7))).toEqual({
            name: "providerDetail",
            providerId: 7,
        });
        expect(match(hrefs.providerEdit(7))).toEqual({
            name: "providerEdit",
            providerId: 7,
        });
        expect(match(hrefs.models(null))).toEqual({
            name: "models",
            providerFilter: null,
        });
        expect(match(hrefs.models(7))).toEqual({
            name: "models",
            providerFilter: 7,
        });
        expect(match(hrefs.modelNew(null))).toEqual({
            name: "modelNew",
            providerFilter: null,
            upstreamId: null,
        });
        expect(match(hrefs.modelNew(7))).toEqual({
            name: "modelNew",
            providerFilter: 7,
            upstreamId: null,
        });
        expect(match(hrefs.modelNew(7, "gpt-x"))).toEqual({
            name: "modelNew",
            providerFilter: 7,
            upstreamId: "gpt-x",
        });
        expect(match(hrefs.apiKeys())).toEqual({
            name: "apiKeys",
        });
        expect(match(hrefs.apiKeyNew())).toEqual({
            name: "apiKeyNew",
        });
        expect(match(hrefs.apiKeyDetail(7))).toEqual({
            name: "apiKeyDetail",
            apiKeyId: 7,
        });
        expect(match(hrefs.apiKeyEdit(7))).toEqual({
            name: "apiKeyEdit",
            apiKeyId: 7,
        });
        expect(match(hrefs.modelDetail(9))).toEqual({
            name: "modelDetail",
            modelId: 9,
        });
        expect(match(hrefs.modelEdit(9))).toEqual({
            name: "modelEdit",
            modelId: 9,
        });
        expect(match(hrefs.usage())).toEqual({
            name: "usage",
            filter: {
                window: "all",
                providerId: "",
                modelId: "",
                apiKeyId: "",
                endpointType: "",
                outcome: "",
                streamChoice: "",
                statusInput: "",
            },
        });
        expect(match(hrefs.usageQuery())).toEqual({
            name: "usageQuery",
            subject: null,
        });
        expect(
            match(
                hrefs.usageQuery({
                    dimension: "provider",
                    id: 7,
                }),
            ),
        ).toEqual({
            name: "usageQuery",
            subject: {
                dimension: "provider",
                id: 7,
            },
        });
        expect(
            match(
                hrefs.usageQuery({
                    dimension: "model",
                    id: 9,
                }),
            ),
        ).toEqual({
            name: "usageQuery",
            subject: {
                dimension: "model",
                id: 9,
            },
        });
        expect(
            match(
                hrefs.usageQuery({
                    dimension: "api_key",
                    id: 3,
                }),
            ),
        ).toEqual({
            name: "usageQuery",
            subject: {
                dimension: "api_key",
                id: 3,
            },
        });
    });
});

describe("routeTitle", () => {
    it("names every payload-less screen over the product", () => {
        expect(routeTitle(match("/"))).toBe("Overview · JanusGate");
        expect(routeTitle(match("/providers"))).toBe("Providers · JanusGate");
        expect(routeTitle(match("/providers/new"))).toBe("Create provider · JanusGate");
        expect(routeTitle(match("/models"))).toBe("Models · JanusGate");
        expect(routeTitle(match("/models/new"))).toBe("Create model · JanusGate");
        expect(routeTitle(match("/api-keys"))).toBe("API keys · JanusGate");
        expect(routeTitle(match("/api-keys/new"))).toBe("Create API key · JanusGate");
        expect(routeTitle(match("/usage"))).toBe("Usage · JanusGate");
        expect(routeTitle(match("/usage/query"))).toBe("Usage aggregates · JanusGate");
        expect(routeTitle(match("/panel"))).toBe("Page not found · JanusGate");
    });

    it("names the row a route addresses by its identifier", () => {
        // The shell knows only the address's row identifier — the row's label
        // is the screen read's to resolve — so the detail and edit titles
        // carry the identifier the route itself carries.
        expect(routeTitle(match("/providers/12"))).toBe("Provider #12 · JanusGate");
        expect(routeTitle(match("/providers/12/edit"))).toBe("Edit provider #12 · JanusGate");
        expect(routeTitle(match("/models/9"))).toBe("Model #9 · JanusGate");
        expect(routeTitle(match("/models/9/edit"))).toBe("Edit model #9 · JanusGate");
        expect(routeTitle(match("/api-keys/7"))).toBe("API key #7 · JanusGate");
        expect(routeTitle(match("/api-keys/7/edit"))).toBe("Edit API key #7 · JanusGate");
    });
});

describe("currentLocation", () => {
    // Solid settles a signal write on a microtask, so a dispatched popstate's
    // write — or the absence of one — becomes observable only after a drain.

    // One traversal step: the address moves first, then the browser fires
    // popstate — the event the router's history listener answers. pushState
    // itself fires no popstate, so the dispatch below is the simulated one.
    async function traverse(target: string): Promise<void> {
        window.history.pushState(null, "", target);
        window.dispatchEvent(new Event("popstate"));
        await drainWrites();
    }

    it("does not notify when a popstate carries the address already held", async () => {
        await traverse("/");
        const before = currentLocation();

        window.dispatchEvent(new Event("popstate"));
        await drainWrites();

        // The signal's equality is reference equality, so an unchanged
        // reference is exactly "did not notify": the listener wrote nothing.
        expect(currentLocation()).toBe(before);
    });

    it("does not notify when a popstate moves only the hash", async () => {
        await traverse("/");
        const before = currentLocation();

        // The "Skip to content" link's traversal: the hash moves, the route
        // address — pathname and search — does not.
        await traverse("#main");

        expect(currentLocation()).toBe(before);
    });

    it("notifies when a popstate carries a different address", async () => {
        await traverse("/");
        const before = currentLocation();

        await traverse("/providers/12");

        expect(currentLocation()).toEqual({
            pathname: "/providers/12",
            search: "",
        });
        expect(currentLocation()).not.toBe(before);
    });

    it("reads a trailing-slash address as the address already held", async () => {
        await traverse("/");

        // A deep link may carry the trailing slash; the location's read point
        // strips it, so the signal speaks the matcher's and the builders'
        // slash-less spelling.
        await traverse("/providers/12/");

        expect(currentLocation()).toEqual({
            pathname: "/providers/12",
            search: "",
        });
        expect(isCurrentAddress("/providers/12")).toBe(true);
        expect(isCurrentAddress("/providers/13")).toBe(false);

        // A traversal between the two spellings carries the address already
        // held: no notification, no keyed remount.
        const before = currentLocation();
        await traverse("/providers/12");

        expect(currentLocation()).toBe(before);
    });
});
