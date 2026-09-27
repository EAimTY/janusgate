import { render } from "@solidjs/web";
import { afterEach, describe, expect, it } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { ProvidersScreen } from "/src/screens/providers-screen";
import {
    bodyText,
    providerRow,
    settle,
    stubListings,
    stubManagementApi,
    typeInto,
} from "./screen-test-support";

/** A management API whose listings answer from the given rows; every other route rejects. */
function screenApi(options: {
    readonly providers: ReadonlyArray<ProviderResponse>;
    readonly models: ReadonlyArray<ModelResponse>;
}): ManagementApi {
    return stubManagementApi(stubListings(options));
}

/** The provider search field, named by its aria-label. */
function searchInput(): HTMLInputElement {
    const input = document.querySelector('input[aria-label="Search providers"]');

    if (input instanceof HTMLInputElement) {
        return input;
    }

    throw new Error("No provider search input");
}

describe("ProvidersScreen search", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

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
                                    model_prefix: "mix-",
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

        typeInto(searchInput(), "openai");
        await settle();

        expect(bodyText()).toContain("mix-ops");
        expect(bodyText()).not.toContain("No providers match");

        typeInto(searchInput(), "zzz");
        await settle();

        expect(bodyText()).toContain('No providers match "zzz"');
    });
});
