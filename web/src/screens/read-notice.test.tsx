import type { JSX } from "@solidjs/web";
import { render } from "@solidjs/web";
import { describe, expect, it } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import type { ListProvidersResponse } from "/src/management-api/generated/v0";
import { ErrorCode } from "/src/management-api/generated/v0";
import { createScreenRead, readOne } from "/src/screens/read";
import { ReadNotice } from "/src/screens/read-notice";
import { createDisposals, gatewayFailure, settle, stubManagementApi } from "./screen-test-support";

/**
 * A screen body reduced to its standing notice: the read harness over one
 * listing, with the screen's own reload handed to the notice as the failed
 * first read's retry where the test wires it.
 */
function NoticeScreen(props: {
    readonly api: ManagementApi;
    readonly isRetryWired: boolean;
}): JSX.Element {
    const read = createScreenRead({
        load: async (signal) => {
            await readOne(signal, {
                run: (cycle) => props.api.listProviders(cycle),
            });
        },
    });

    return (
        <ReadNotice
            read={read}
            retry={props.isRetryWired ? read.reload : undefined}
        />
    );
}

/**
 * A provider listing whose first answer is held until the test hands it one:
 * the window the failed first read and its recovery live in.
 */
function heldListing(): {
    readonly listProviders: ManagementApi["listProviders"];
    readonly answer: () => void;
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
        answer: () =>
            deliver?.({
                providers: [],
            }),
        refuse: (error) => rejectHeld?.(error),
    };
}

/** The notice's read-again action, by its label. */
function readAgainButton(): HTMLButtonElement {
    for (const candidate of document.querySelectorAll("button")) {
        if (candidate.textContent?.includes("Read again") === true) {
            return candidate as HTMLButtonElement;
        }
    }

    throw new Error("No button reading Read again");
}

describe("ReadNotice", () => {
    const disposes = createDisposals();

    function mount(listing: ManagementApi["listProviders"], isRetryWired: boolean): void {
        disposes.push(
            render(
                () => (
                    <NoticeScreen
                        api={stubManagementApi({
                            listProviders: listing,
                        })}
                        isRetryWired={isRetryWired}
                    />
                ),
                document.body,
            ),
        );
    }

    it("offers the failed first read a Read again action that reruns the screen's read in place", async () => {
        const held = heldListing();
        mount(held.listProviders, true);
        await settle();

        // The first read is still in flight: no failure stands, so no
        // read-again action stands with it.
        expect(() => readAgainButton()).toThrow("No button reading Read again");

        held.refuse(gatewayFailure("the listing dropped", ErrorCode.UpstreamError, 502));
        await settle();

        // The failure's diagnosis stands, and the read-again action with it —
        // the same wording the detail and edit frame's own action carries.
        expect(document.body.textContent).toContain(
            "The upstream provider refused the call or was unreachable.",
        );
        expect(document.body.textContent).toContain("the listing dropped");
        expect(readAgainButton()).toBeInstanceOf(HTMLButtonElement);

        readAgainButton().click();

        // The re-read cycle runs under the still-standing failure, and the
        // action waits for the cycle it started.
        await settle();
        expect(document.body.textContent).toContain("the listing dropped");
        expect(readAgainButton().disabled).toBe(true);

        held.answer();
        await settle();

        // The read landed: the read-failure notice it diagnoses is cleared,
        // and the action stands down with it.
        expect(document.body.textContent).not.toContain("the listing dropped");
        expect(() => readAgainButton()).toThrow("No button reading Read again");
    });

    it("states a failed first read alone where the screen wires no retry", async () => {
        const held = heldListing();
        mount(held.listProviders, false);
        await settle();

        held.refuse(gatewayFailure("the listing dropped", ErrorCode.UpstreamError, 502));
        await settle();

        // The diagnosis stands with no action beside it: the slot is the
        // screen's to wire, and an unwired notice stays diagnosis only.
        expect(document.body.textContent).toContain("the listing dropped");
        expect(() => readAgainButton()).toThrow("No button reading Read again");
    });
});
