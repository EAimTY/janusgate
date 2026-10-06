import { render } from "@solidjs/web";
import { describe, expect, it } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import type { ApiKeyResponse } from "/src/management-api/generated/v0";
import {
    apiKeyRow,
    bodyText,
    createDisposals,
    searchInput,
    settle,
    stubManagementApi,
    typeInto,
} from "/src/test-support/management-api";
import { ApiKeysScreen } from "./api-keys-screen";

/** Builds one listing row whose secret spells its own name out. */
function keyRow(id: number, name: string, isEnabled: boolean): ApiKeyResponse {
    return apiKeyRow({
        id,
        name,
        key: `sk-${name}`,
        is_enabled: isEnabled,
    });
}

/**
 * Five keys, two of them enabled, whose names share no substring with one
 * another, so a search needle names at most one row.
 */
const keys: ReadonlyArray<ApiKeyResponse> = [
    keyRow(1, "alpha-on", true),
    keyRow(2, "beta-off", false),
    keyRow(3, "gamma-off", false),
    keyRow(4, "delta-on", true),
    keyRow(5, "epsilon-off", false),
];

/** A management API whose key listing answers from the fixture; every other route rejects. */
function screenApi(): ManagementApi {
    return stubManagementApi({
        listModelRoutes: () =>
            Promise.resolve({
                model_routes: [],
            }),
        listApiKeys: () =>
            Promise.resolve({
                api_keys: [
                    ...keys,
                ],
            }),
    });
}

describe("ApiKeysScreen count line", () => {
    const disposes = createDisposals();

    function mount(): void {
        disposes.push(render(() => <ApiKeysScreen api={screenApi()} />, document.body));
    }

    it("counts the enabled rows the search leaves visible, and the whole listing once it clears", async () => {
        mount();
        await settle();

        // Without a search every row is visible, so the line describes the
        // whole listing.
        expect(bodyText()).toContain("2 of 5 keys enabled");

        // Under a standing search the numerator and the denominator diverge,
        // so the line states both facts instead of reading as visible over
        // total, the shape the models screen's sentence means.
        typeInto(searchInput("Search API keys"), "alpha");
        await settle();
        expect(bodyText()).toContain("1 key shown · 1 of 5 keys enabled");

        // The numerator counts enabled rows among the visible ones, not the
        // visible rows themselves: the single row a disabled match leaves is
        // counted as off, and the denominator stays the whole listing.
        typeInto(searchInput("Search API keys"), "beta");
        await settle();
        expect(bodyText()).toContain("1 key shown · 0 of 5 keys enabled");

        typeInto(searchInput("Search API keys"), "");
        await settle();
        expect(bodyText()).toContain("2 of 5 keys enabled");
    });

    it("leaves the counting to the no-match notice when a search matches nothing", async () => {
        mount();
        await settle();

        typeInto(searchInput("Search API keys"), "zzz");
        await settle();

        expect(bodyText()).toContain('No API keys match "zzz"');
        expect(bodyText()).not.toContain("keys enabled");
    });

    it("names each row's copy button after the key it copies", async () => {
        mount();
        await settle();

        // Every row repeats the visible Copy label, so the accessible name —
        // not the shared label — is what tells the keys apart.
        const labels = Array.from(
            document.querySelectorAll('button[aria-label^="Copy the secret of "]'),
        ).map((button) => button.getAttribute("aria-label"));

        expect(labels).toEqual([
            "Copy the secret of alpha-on",
            "Copy the secret of beta-off",
            "Copy the secret of gamma-off",
            "Copy the secret of delta-on",
            "Copy the secret of epsilon-off",
        ]);
    });
});
