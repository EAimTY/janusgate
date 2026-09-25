import type { JSX } from "@solidjs/web";
import { render } from "@solidjs/web";
import { createSignal } from "solid-js";
import { afterEach, describe, expect, it } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import { ManagementApiError } from "/src/management-api/errors";
import type { ListProvidersResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { ErrorCode } from "/src/management-api/generated/v0";
import { createPinnedRow, createScreenRead, isSameProviderRow, readOne } from "/src/screens/read";
import { ScreenFrame } from "/src/screens/screen-frame";
import { settle, stubManagementApi } from "./screen-test-support";

const provider: ProviderResponse = {
    type: "api_key",
    provider_definition_name: "generic-openai-responses",
    provider_definition_version: 1,
    format: "openai_response",
    id: 3,
    name: "alpha",
    model_prefix: "alpha-",
    is_enabled: true,
    base_url: "https://alpha.example",
};

/**
 * A provider listing whose first answer is held until the test hands it one:
 * exactly the window the frame's first-read branch lives in. The test either
 * answers the listing with rows or refuses it with a failure, as the gateway
 * would.
 */
function heldListing(): {
    readonly listProviders: ManagementApi["listProviders"];
    readonly answer: (rows: ReadonlyArray<ProviderResponse>) => void;
    readonly refuse: (error: unknown) => void;
} {
    let deliver: ((result: ListProvidersResponse) => void) | null = null;
    let rejectHeld: ((error: unknown) => void) | null = null;

    return {
        listProviders: () =>
            new Promise((resolve, reject) => {
                deliver = resolve;
                rejectHeld = reject;
            }),
        answer: (rows) =>
            deliver?.({
                providers: [
                    ...rows,
                ],
            }),
        refuse: (error) => rejectHeld?.(error),
    };
}

/**
 * A row screen as the detail and edit screens are built: a read cycle over a
 * listing, the row the screen addresses drawn from it, and the frame's three
 * branches around that row with distinct marker text per branch.
 */
function FrameScreen(props: {
    readonly api: ManagementApi;
    readonly providerId: number;
}): JSX.Element {
    const [providers, setProviders] = createSignal<ReadonlyArray<ProviderResponse>>([]);

    const read = createScreenRead({
        load: async (signal) => {
            const listing = await readOne(signal, {
                run: (cycle) => props.api.listProviders(cycle),
            });
            setProviders(listing.providers);
        },
    });

    const row = createPinnedRow(
        () => providers().find((candidate) => candidate.id === props.providerId) ?? null,
        isSameProviderRow,
    );

    return (
        <ScreenFrame
            read={read}
            row={row}
            firstReadHeader={<h2>the screen's own header</h2>}
            notFound={<p>the not-found state</p>}
        >
            {(current) => <p>{`the row content of ${current.name}`}</p>}
        </ScreenFrame>
    );
}

/** The skeleton the frame renders in its first-read window, by its placeholder construction. */
function skeleton(): HTMLDivElement | null {
    return document.body.querySelector("div[aria-hidden='true'].wash");
}

/** The frame's read-again action of the failed first read, by its label. */
function readAgainButton(): HTMLButtonElement {
    for (const candidate of document.querySelectorAll("button")) {
        if (candidate.textContent?.includes("Read again") === true) {
            return candidate as HTMLButtonElement;
        }
    }

    throw new Error("No button reading Read again");
}

describe("ScreenFrame", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    function mount(listing: ManagementApi["listProviders"]): void {
        disposes.push(
            render(
                () => (
                    <FrameScreen
                        api={stubManagementApi({
                            listProviders: listing,
                        })}
                        providerId={3}
                    />
                ),
                document.body,
            ),
        );
    }

    it("renders the screen's own header above the skeleton through the first read", async () => {
        const held = heldListing();
        mount(held.listProviders);
        await settle();

        // The first read is in flight: the header slot the screen supplies is
        // up, and the frame's skeleton stands under it over the empty body.
        expect(document.body.textContent).toContain("the screen's own header");
        expect(skeleton()).not.toBeNull();

        held.answer([
            provider,
        ]);
        await settle();

        // The read landed: the first-read window closed with the skeleton and
        // the header slot, and the row's content took over.
        expect(skeleton()).toBeNull();
        expect(document.body.textContent).not.toContain("the screen's own header");
        expect(document.body.textContent).toContain("the row content of alpha");
    });

    it("keeps the screen's own header beside the standing notice a failed first read states", async () => {
        const held = heldListing();
        mount(held.listProviders);
        await settle();
        expect(skeleton()).not.toBeNull();

        held.refuse(
            new ManagementApiError("gateway", "the listing dropped", {
                errno: ErrorCode.UpstreamError,
                httpStatus: 502,
            }),
        );
        await settle();

        // The failure's diagnosis stands where every screen states it, the
        // skeleton that would cover it is gone, and the screen's own header
        // stays up beside it — the page keeps its identity and way back —
        // with the read-again action offering the reload.
        expect(document.body.textContent).toContain(
            "The upstream provider refused the call or was unreachable.",
        );
        expect(document.body.textContent).toContain("the listing dropped");
        expect(skeleton()).toBeNull();
        expect(document.body.textContent).toContain("the screen's own header");
        expect(readAgainButton()).toBeInstanceOf(HTMLButtonElement);
    });

    it("reads again from the failed first read and recovers onto the row", async () => {
        const held = heldListing();
        mount(held.listProviders);
        await settle();

        held.refuse(
            new ManagementApiError("gateway", "the listing dropped", {
                errno: ErrorCode.UpstreamError,
                httpStatus: 502,
            }),
        );
        await settle();

        readAgainButton().click();

        // The re-read cycle runs under the still-standing failure: the
        // header stays up, the skeleton stays closed rather than stacking
        // over the failure, and the read-again action waits for the cycle
        // it started.
        await settle();
        expect(document.body.textContent).toContain("the screen's own header");
        expect(document.body.textContent).toContain("the listing dropped");
        expect(skeleton()).toBeNull();
        expect(readAgainButton().disabled).toBe(true);

        held.answer([
            provider,
        ]);
        await settle();

        // The read landed this time: the row's content takes over, and the
        // failure, the header slot, and the read-again action stand down.
        expect(document.body.textContent).toContain("the row content of alpha");
        expect(document.body.textContent).not.toContain("the listing dropped");
        expect(document.body.textContent).not.toContain("the screen's own header");
        expect(() => readAgainButton()).toThrow("No button reading Read again");
    });

    it("renders the not-found slot when the read lands but no row answers the address", async () => {
        const held = heldListing();
        mount(held.listProviders);
        await settle();

        held.answer([]);
        await settle();

        // The read succeeded but the addressed row is gone: the screen's
        // not-found slot renders, with neither skeleton nor row content.
        expect(document.body.textContent).toContain("the not-found state");
        expect(skeleton()).toBeNull();
        expect(document.body.textContent).not.toContain("the row content of alpha");
    });

    it("renders the children keyed to the row when the read lands on one", async () => {
        const held = heldListing();
        mount(held.listProviders);
        await settle();

        held.answer([
            provider,
        ]);
        await settle();

        // The present row is handed to the children keyed: the screen's
        // content renders on it, and the not-found state stays out.
        expect(document.body.textContent).toContain("the row content of alpha");
        expect(document.body.textContent).not.toContain("the not-found state");
        expect(skeleton()).toBeNull();
    });
});
