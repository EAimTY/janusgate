import { render } from "@solidjs/web";
import { afterEach, describe, expect, it } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import type { ApiKeyResponse } from "/src/management-api/generated/v0";
import { ApiKeysScreen } from "/src/screens/api-keys-screen";
import { bodyText, settle, stubManagementApi, typeInto } from "./screen-test-support";

/** Builds one listing row whose secret spells its own name out. */
function keyRow(id: number, name: string, isEnabled: boolean): ApiKeyResponse {
    return {
        id,
        name,
        key: `sk-${name}`,
        is_enabled: isEnabled,
        created_at: 1_700_000_000_000_000,
    };
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
        listApiKeys: () =>
            Promise.resolve({
                api_keys: [
                    ...keys,
                ],
            }),
    });
}

/** The keys search field, named by its aria-label. */
function searchInput(): HTMLInputElement {
    const input = document.querySelector('input[aria-label="Search API keys"]');

    if (input instanceof HTMLInputElement) {
        return input;
    }

    throw new Error("No API keys search input");
}

describe("ApiKeysScreen count line", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    function mount(): void {
        disposes.push(render(() => <ApiKeysScreen api={screenApi()} />, document.body));
    }

    it("counts the enabled rows the search leaves visible, and the whole listing once it clears", async () => {
        mount();
        await settle();

        // Without a search every row is visible, so the line describes the
        // whole listing.
        expect(bodyText()).toContain("2 of 5 keys enabled");

        typeInto(searchInput(), "alpha");
        await settle();
        expect(bodyText()).toContain("1 of 5 keys enabled");

        // The numerator counts enabled rows among the visible ones, not the
        // visible rows themselves: the single row a disabled match leaves is
        // counted as off, and the denominator stays the whole listing.
        typeInto(searchInput(), "beta");
        await settle();
        expect(bodyText()).toContain("0 of 5 keys enabled");

        typeInto(searchInput(), "");
        await settle();
        expect(bodyText()).toContain("2 of 5 keys enabled");
    });

    it("leaves the counting to the no-match notice when a search matches nothing", async () => {
        mount();
        await settle();

        typeInto(searchInput(), "zzz");
        await settle();

        expect(bodyText()).toContain('No API keys match "zzz"');
        expect(bodyText()).not.toContain("keys enabled");
    });
});
