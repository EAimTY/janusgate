import { describe, expect, it } from "vitest";
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

    it("addresses one provider by its identifier", () => {
        expect(match("/providers/12")).toEqual({
            name: "providerDetail",
            providerId: 12,
        });
        expect(match("/providers/12/")).toEqual({
            name: "providerDetail",
            providerId: 12,
        });
    });

    it("reads a provider segment that is not a row identifier as not found", () => {
        expect(match("/providers/codex")).toEqual({
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
        });
        expect(match("/models/new?provider=4")).toEqual({
            name: "modelNew",
            providerFilter: 4,
        });
        expect(match("/models/new?provider=codex")).toEqual({
            name: "modelNew",
            providerFilter: null,
        });
    });

    it("addresses the api-keys list, creation, and detail", () => {
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
        expect(match("/api-keys/codex")).toEqual({
            name: "notFound",
        });
    });

    it("addresses one model by its provider and upstream identifiers", () => {
        expect(match("/models/4/gpt-4o")).toEqual({
            name: "modelDetail",
            providerId: 4,
            upstreamId: "gpt-4o",
        });
    });

    it("decodes an encoded upstream identifier", () => {
        expect(match("/models/4/gpt%204o%20mini")).toEqual({
            name: "modelDetail",
            providerId: 4,
            upstreamId: "gpt 4o mini",
        });
        expect(match("/models/4/org%2Fmodel")).toEqual({
            name: "modelDetail",
            providerId: 4,
            upstreamId: "org/model",
        });
    });

    it("reads a model address without both identifiers as not found", () => {
        expect(match("/models/4")).toEqual({
            name: "notFound",
        });
        expect(match("/models/codex/gpt-4o")).toEqual({
            name: "notFound",
        });
        expect(match("/models/4/%")).toEqual({
            name: "notFound",
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
        });
        expect(match(hrefs.modelNew(7))).toEqual({
            name: "modelNew",
            providerFilter: 7,
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
        expect(match(hrefs.modelDetail(7, "gpt-4o"))).toEqual({
            name: "modelDetail",
            providerId: 7,
            upstreamId: "gpt-4o",
        });
        expect(match(hrefs.modelDetail(7, "org/model mini"))).toEqual({
            name: "modelDetail",
            providerId: 7,
            upstreamId: "org/model mini",
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
