import { render } from "@solidjs/web";
import { describe, expect, it } from "vitest";
import {
    bodyText,
    buttonWithText,
    createDisposals,
    settle,
    stubManagementApi,
} from "/src/test-support/management-api";
import { ProviderPoolsScreen } from "./provider-pools-screen";

describe("ProviderPoolsScreen failed first read", () => {
    const disposes = createDisposals();

    it("states a failed listing's diagnosis and recovers in place through Read again", async () => {
        let isListingFailing = true;
        let listingReads = 0;
        disposes.push(
            render(
                () => (
                    <ProviderPoolsScreen
                        api={stubManagementApi({
                            listProviderPools: () => {
                                listingReads += 1;

                                return isListingFailing
                                    ? Promise.reject(new Error("the pools listing refused"))
                                    : Promise.resolve({
                                          provider_pools: [],
                                      });
                            },
                        })}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        // The failed first read states its diagnosis on the standing notice
        // with the Read again action beside it — the screen never hangs on
        // its skeleton with the failure swallowed (README §6.3).
        expect(bodyText()).toContain("the pools listing refused");
        expect(buttonWithText("Read again").disabled).toBe(false);

        // The action re-runs the screen's own read in place: the listing
        // answers this time and the notice is recovered away.
        isListingFailing = false;
        buttonWithText("Read again").click();
        await settle();

        expect(listingReads).toBe(2);
        expect(bodyText()).not.toContain("the pools listing refused");
    });
});
