import { describe, expect, it } from "vitest";
import { EMPTY_USAGE_FILTER_DRAFT } from "/src/screens/usage-filter";
import { hrefs, matchAppRoute, parseRowIdentifier } from "./router";

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
        });
        expect(match("/usage/query/")).toEqual({
            name: "usageQuery",
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
        });
    });
});

describe("parseRowIdentifier", () => {
    it("accepts plain decimal integers", () => {
        expect(parseRowIdentifier("12")).toBe(12);
    });

    it("rejects everything else", () => {
        expect(parseRowIdentifier(null)).toBeNull();
        expect(parseRowIdentifier("")).toBeNull();
        expect(parseRowIdentifier("codex")).toBeNull();
        expect(parseRowIdentifier("12x")).toBeNull();
        expect(parseRowIdentifier("-3")).toBeNull();
        expect(parseRowIdentifier("1.5")).toBeNull();
    });
});
