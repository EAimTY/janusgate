import { render } from "@solidjs/web";
import { describe, expect, it } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import type { ApiKeyResponse } from "/src/management-api/generated/v0";
import { ApiKeyDetailScreen } from "/src/screens/api-key-detail-screen";
import { bodyText, createDisposals, settle, stubManagementApi } from "./screen-test-support";

/** One serving key whose secret spells its own name. */
function keyRow(isEnabled: boolean): ApiKeyResponse {
    return {
        id: 7,
        name: "ci-runner",
        key: "sk-ci-runner",
        is_enabled: isEnabled,
        created_at: 1_700_000_000_000_000,
    };
}

/** A management API whose key listing answers the fixture; usage answers empty. */
function screenApi(apiKey: ApiKeyResponse): ManagementApi {
    return stubManagementApi({
        listModelRoutes: () =>
            Promise.resolve({
                model_routes: [],
            }),
        listApiKeys: () =>
            Promise.resolve({
                api_keys: [
                    apiKey,
                ],
            }),
        queryUsage: () =>
            Promise.resolve({
                buckets: [],
            }),
    });
}

describe("ApiKeyDetailScreen facts", () => {
    const disposes = createDisposals();

    function mount(apiKey: ApiKeyResponse): void {
        disposes.push(
            render(
                () => (
                    <ApiKeyDetailScreen
                        api={screenApi(apiKey)}
                        apiKeyId={7}
                    />
                ),
                document.body,
            ),
        );
    }

    it("speaks one enabled pair per page — the header badge and the Details card both read Enabled", async () => {
        mount(keyRow(true));
        await settle();

        // The detail-page vocabulary: both surfaces count one "Enabled" each,
        // and the tight list-row value never leaks onto the page.
        expect(bodyText().split("Enabled").length - 1).toBe(2);
        expect(bodyText()).not.toContain("Off");
    });

    it("names a disabled key Disabled on both surfaces, never Off", async () => {
        mount(keyRow(false));
        await settle();

        expect(bodyText().split("Disabled").length - 1).toBe(2);
        expect(bodyText()).not.toContain("Enabled");
        expect(bodyText()).not.toContain("Off");
    });

    it("points the freshly copied key's client home, where the endpoint URLs live", async () => {
        mount(keyRow(true));
        await settle();

        // The Secret card ends the provider → key → client journey: its one
        // quiet line names where a client points and links the Overview,
        // whose Endpoints card carries the surface URLs.
        expect(bodyText()).toContain("Point your client at an endpoint");
        expect(document.querySelector('a[href="/"]')?.textContent).toBe("Overview");
    });
});
