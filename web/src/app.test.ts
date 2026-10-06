import { render } from "@solidjs/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import { App, guardLeaveOnGuardedSubjects } from "./app";
import { connectionService } from "./connection/app-service";
import { type LeaveGuardSeat, registerLeaveGuardSubject } from "./leave-guard-registry";
import { hrefs, navigate } from "./router";
import {
    bodyText,
    buttonWithText,
    drainWrites,
    labeledInput,
    paragraphWithText,
    settle,
} from "./test-support/management-api";

/**
 * A plain stand-in for the leave event: only the surface the guard writes.
 * jsdom implements no document lifecycle, so no real `beforeunload` event can
 * be dispatched here — the guard's decision is pinned against this stand-in,
 * and the listener's registration itself is the browser's standard API.
 */
function leaveStandIn(): {
    readonly event: {
        readonly preventDefault: () => void;
        returnValue: string;
    };
    readonly cancelCount: () => number;
} {
    let cancellations = 0;

    return {
        event: {
            preventDefault: (): void => {
                cancellations += 1;
            },
            returnValue: "",
        },
        cancelCount: (): number => cancellations,
    };
}

// Solid settles a signal write on a microtask, so a seat's report becomes
// observable to the guard only after a drain of the queue — exactly the gap
// the real event-time read always sits beyond.

// The registry is module state: every test gives the seats it took back, so
// no test leaves the guard armed for the next one.
const takenSeats: Array<LeaveGuardSeat> = [];

afterEach(() => {
    for (const seat of takenSeats) {
        seat.release();
    }
    takenSeats.length = 0;
});

function takeSeat(): LeaveGuardSeat {
    const seat = registerLeaveGuardSubject();
    takenSeats.push(seat);
    return seat;
}

describe("guardLeaveOnGuardedSubjects", () => {
    it("cancels nothing while the registry is clean", () => {
        const { event, cancelCount } = leaveStandIn();

        guardLeaveOnGuardedSubjects(event);

        expect(cancelCount()).toBe(0);
        expect(event.returnValue).toBe("");
    });

    it("marks the leave for the browser's confirmation while a form stands dirty", async () => {
        const { event, cancelCount } = leaveStandIn();
        takeSeat().reportGuarded(true);
        await drainWrites();

        guardLeaveOnGuardedSubjects(event);

        expect(cancelCount()).toBe(1);
        expect(event.returnValue).toBe("");
    });

    it("reads the registry live — the one handler answers the dirt of the moment", async () => {
        const { event, cancelCount } = leaveStandIn();
        const seat = takeSeat();

        seat.reportGuarded(true);
        await drainWrites();
        guardLeaveOnGuardedSubjects(event);
        expect(cancelCount()).toBe(1);

        // The dirt clears: the same listener, asked again, cancels nothing —
        // a captured snapshot would keep blocking.
        seat.reportGuarded(false);
        await drainWrites();
        guardLeaveOnGuardedSubjects(event);
        expect(cancelCount()).toBe(1);

        // And the dirt returns.
        seat.reportGuarded(true);
        await drainWrites();
        guardLeaveOnGuardedSubjects(event);
        expect(cancelCount()).toBe(2);
    });

    it("stays disarmed once the dirty form's seat is given up", async () => {
        const { event, cancelCount } = leaveStandIn();
        const seat = takeSeat();
        seat.reportGuarded(true);

        // The screen went away without clearing its dirt first: the seat's
        // disposal must still leave the guard disarmed.
        seat.release();
        await drainWrites();
        guardLeaveOnGuardedSubjects(event);

        expect(cancelCount()).toBe(0);
    });
});

/** A management API answer with the given status and JSON body. */
function jsonResponse(status: number, payload: unknown): Promise<Response> {
    return Promise.resolve(
        new Response(JSON.stringify(payload), {
            status,
            headers: {
                "Content-Type": "application/json",
            },
        }),
    );
}

describe("App disconnect confirm", () => {
    const gatewayAddress = "https://gateway.example.com";
    let dispose: (() => void) | undefined;

    afterEach(() => {
        dispose?.();
        dispose = undefined;
        vi.unstubAllGlobals();
        window.localStorage?.clear();
        // The service singleton outlives one test, and one forget cannot reset
        // it — the forgotten address stays as the setup seed — so the second
        // forget runs from that unconnected state and restores the initial
        // one. The connection screen suite pins the same dance.
        connectionService.forget();
        connectionService.forget();
    });

    it("asks its consequence in the armed state, and the armed action returns to setup", async () => {
        // The probe answers while the required key-policy inventory reads stay pending.
        vi.stubGlobal("fetch", (input: RequestInfo | URL) =>
            String(input).endsWith("/providers:list")
                ? jsonResponse(200, {
                      providers: [],
                  })
                : new Promise<never>(() => {}),
        );

        await connectionService.connect({
            baseUrl: gatewayAddress,
            token: "any token",
        });

        navigate(hrefs.apiKeyNew());
        dispose = render(App, document.body);
        await settle();

        expect(connectionService.currentConnection()).toEqual({
            baseUrl: gatewayAddress,
            token: "any token",
        });

        // Key creation now owns required inventory reads, including the shell refresh action.
        expect(buttonWithText("Refresh").disabled).toBe(false);

        // Arming states what arming costs — the bar's one armed control
        // speaks the same confirm grammar as every in-page confirm: the
        // question, the action, the way back. Focus moves onto the armed
        // question, so the question is what a screen reader announces next.
        buttonWithText("Disconnect").focus();
        buttonWithText("Disconnect").click();
        await settle();

        expect(bodyText()).toContain(
            "Disconnect and forget the stored token? Reconnecting means entering it again.",
        );
        expect(document.activeElement).toBe(
            paragraphWithText("Disconnect and forget the stored token?"),
        );

        // Keep steps back to the plain control — focus returned with it.
        buttonWithText("Keep").click();
        await settle();

        expect(bodyText()).not.toContain("Disconnect and forget the stored token?");
        expect(bodyText()).toContain("Disconnect");
        expect(document.activeElement).toBe(buttonWithText("Disconnect"));

        // And the armed action forgets: setup reopens, seeded with the address
        // the forgotten connection served — never the token. In the armed
        // state the confirm is the only "Disconnect" button — the trigger it
        // replaced is unmounted.
        buttonWithText("Disconnect").click();
        await settle();
        buttonWithText("Disconnect").click();
        await settle();

        expect(connectionService.currentConnection()).toBeNull();
        expect(labeledInput("Gateway address").value).toBe(gatewayAddress);
    });
});
