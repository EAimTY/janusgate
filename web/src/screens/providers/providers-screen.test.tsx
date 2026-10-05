import { render } from "@solidjs/web";
import { describe, expect, it } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import {
    bodyText,
    createDisposals,
    providerRow,
    searchInput,
    settle,
    stubListings,
    stubManagementApi,
    typeInto,
} from "/src/test-support/management-api";
import { ProvidersScreen } from "./providers-screen";

/** A management API whose listings answer from the given rows; every other route rejects. */
function screenApi(options: {
    readonly providers: ReadonlyArray<ProviderResponse>;
    readonly models: ReadonlyArray<ModelResponse>;
}): ManagementApi {
    return stubManagementApi(stubListings(options));
}

describe("ProvidersScreen search", () => {
    const disposes = createDisposals();

    it("matches the wire format the rows display", async () => {
        disposes.push(
            render(
                () => (
                    <ProvidersScreen
                        api={screenApi({
                            providers: [
                                // A provider whose name and credential kind
                                // share nothing with its wire format's label,
                                // so a search matching it can only be matching
                                // the format the row displays.
                                providerRow({
                                    id: 4,
                                    name: "mix-ops",
                                }),
                            ],
                            models: [],
                        })}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        typeInto(searchInput("Search providers"), "openai");
        await settle();

        expect(bodyText()).toContain("mix-ops");
        expect(bodyText()).not.toContain("No providers match");

        typeInto(searchInput("Search providers"), "zzz");
        await settle();

        expect(bodyText()).toContain('No providers match "zzz"');
    });
});
