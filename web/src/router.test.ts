import { describe, expect, it } from "vitest";
import { hrefs, matchAppRoute } from "./router";

describe("matchAppRoute", () => {
    it("treats the root and the providers path as the providers screen", () => {
        expect(matchAppRoute("/", "")).toEqual({
            name: "providers",
        });

        expect(matchAppRoute("/providers", "")).toEqual({
            name: "providers",
        });
    });

    it("matches the provider creation path before the detail pattern", () => {
        expect(matchAppRoute("/providers/new", "")).toEqual({
            name: "providerCreate",
        });
    });

    it("reads a provider id out of the detail path", () => {
        expect(matchAppRoute("/providers/12", "")).toEqual({
            name: "providerDetail",
            providerId: 12,
        });
    });

    it("tolerates a trailing slash", () => {
        expect(matchAppRoute("/providers/12/", "")).toEqual({
            name: "providerDetail",
            providerId: 12,
        });

        expect(matchAppRoute("/models/", "")).toEqual({
            name: "models",
            providerFilter: null,
        });
    });

    it("reads the model provider filter from the query", () => {
        expect(matchAppRoute("/models", "")).toEqual({
            name: "models",
            providerFilter: null,
        });

        expect(matchAppRoute("/models", "?provider=7")).toEqual({
            name: "models",
            providerFilter: 7,
        });

        expect(matchAppRoute("/models", "?unknown=1")).toEqual({
            name: "models",
            providerFilter: null,
        });

        // A filter addresses no row when it is not a row identifier.
        expect(matchAppRoute("/models", "?provider=all")).toEqual({
            name: "models",
            providerFilter: null,
        });
    });

    it("reads the reauthorization provider from the OAuth path's query", () => {
        expect(matchAppRoute("/oauth/start", "")).toEqual({
            name: "oauthStart",
            reauthorizeProviderId: null,
        });

        expect(matchAppRoute("/oauth/start", "?provider=3")).toEqual({
            name: "oauthStart",
            reauthorizeProviderId: 3,
        });
    });

    it("treats every other path as not found", () => {
        for (const pathname of [
            "/providers/12/models",
            "/providers/new/extra",
            "/Providers",
            "/oauth",
            "/oauth/start/extra",
            "/settings",
        ]) {
            expect(matchAppRoute(pathname, "")).toEqual({
                name: "notFound",
            });
        }
    });

    it("rejects a provider id that is not a safe integer", () => {
        expect(matchAppRoute("/providers/99999999999999999999", "")).toEqual({
            name: "notFound",
        });
    });
});

describe("hrefs", () => {
    it("builds the app's links", () => {
        expect(hrefs.providers()).toBe("/providers");
        expect(hrefs.providerCreate()).toBe("/providers/new");
        expect(hrefs.providerDetail(12)).toBe("/providers/12");
        expect(hrefs.models(null)).toBe("/models");
        expect(hrefs.models(7)).toBe("/models?provider=7");
        expect(hrefs.oauthStart(null)).toBe("/oauth/start");
        expect(hrefs.oauthStart(3)).toBe("/oauth/start?provider=3");
    });

    it("round-trips through the matcher", () => {
        const providerDetail = matchAppRoute(hrefs.providerDetail(12), "");

        expect(providerDetail).toEqual({
            name: "providerDetail",
            providerId: 12,
        });

        const models = matchAppRoute("/models", "?provider=7");

        expect(models).toEqual({
            name: "models",
            providerFilter: 7,
        });

        const oauth = matchAppRoute("/oauth/start", "?provider=3");

        expect(oauth).toEqual({
            name: "oauthStart",
            reauthorizeProviderId: 3,
        });
    });
});
