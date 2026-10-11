import { describe, expect, test } from "vitest";
import { initialConnectionState, reduceConnectionState } from "./state";

/**
 * The connection lifecycle's reducer: events are the only way state changes,
 * every transition is stated explicitly, and an event that does not apply to
 * the current state leaves it untouched — a submission only before a
 * connection, an acceptance only while verifying, a cancellation only while
 * a probe runs, and a rejection from anywhere.
 */

/** The connected state every transition below starts from. */
const CONNECTED = {
    status: "connected" as const,
    connection: {
        baseUrl: "http://gw.test",
        token: "t-1",
    },
};

describe("reduceConnectionState", () => {
    test("starts unconnected with no failure and no forgotten address", () => {
        expect(initialConnectionState).toEqual({
            status: "unconnected",
            failure: null,
            forgottenBaseUrl: null,
        });
    });

    test("lands connected when storage held a pair, pristine otherwise", () => {
        expect(
            reduceConnectionState(initialConnectionState, {
                type: "storedConnectionLoaded",
                connection: CONNECTED.connection,
            }),
        ).toEqual({
            status: "connected",
            connection: CONNECTED.connection,
        });

        expect(
            reduceConnectionState(initialConnectionState, {
                type: "storedConnectionLoaded",
                connection: null,
            }),
        ).toEqual({
            status: "unconnected",
            failure: null,
            forgottenBaseUrl: null,
        });
    });

    test("moves to verifying only from unconnected", () => {
        expect(
            reduceConnectionState(
                {
                    status: "unconnected",
                    failure: null,
                    forgottenBaseUrl: null,
                },
                {
                    type: "connectionSubmitted",
                },
            ),
        ).toEqual({
            status: "verifying",
        });

        // A submission is not meaningful once connected or while a probe runs.
        expect(
            reduceConnectionState(CONNECTED, {
                type: "connectionSubmitted",
            }),
        ).toBe(CONNECTED);
        expect(
            reduceConnectionState(
                {
                    status: "verifying",
                },
                {
                    type: "connectionSubmitted",
                },
            ),
        ).toEqual({
            status: "verifying",
        });
    });

    test("lands connected on acceptance only while verifying", () => {
        const accepted = reduceConnectionState(
            {
                status: "verifying",
            },
            {
                type: "connectionAccepted",
                connection: CONNECTED.connection,
            },
        );

        expect(accepted).toEqual({
            status: "connected",
            connection: CONNECTED.connection,
        });
        expect(
            reduceConnectionState(CONNECTED, {
                type: "connectionAccepted",
                connection: {
                    baseUrl: "http://other.test",
                    token: "t-2",
                },
            }),
        ).toBe(CONNECTED);
    });

    test("lands on setup carrying the refusal, from verifying and connected alike", () => {
        const failure = {
            message: "the gateway refused the token",
            cause: new Error("401"),
            baseUrl: "http://gw.test",
        };

        for (const state of [
            initialConnectionState,
            {
                status: "verifying",
            } as const,
            CONNECTED,
        ]) {
            expect(
                reduceConnectionState(state, {
                    type: "connectionRejected",
                    failure,
                }),
            ).toEqual({
                status: "unconnected",
                failure,
                forgottenBaseUrl: null,
            });
        }
    });

    test("lands a cancellation on setup carrying no failure, only while a probe runs", () => {
        expect(
            reduceConnectionState(
                {
                    status: "verifying",
                },
                {
                    type: "connectionCancelled",
                },
            ),
        ).toEqual({
            status: "unconnected",
            failure: null,
            forgottenBaseUrl: null,
        });

        const untouched = reduceConnectionState(CONNECTED, {
            type: "connectionCancelled",
        });
        expect(untouched).toBe(CONNECTED);
    });

    test("forgets to setup seeding the address the connection served", () => {
        expect(
            reduceConnectionState(CONNECTED, {
                type: "connectionForgotten",
                baseUrl: "http://gw.test",
            }),
        ).toEqual({
            status: "unconnected",
            failure: null,
            forgottenBaseUrl: "http://gw.test",
        });

        // Nothing connected: nothing to seed.
        expect(
            reduceConnectionState(initialConnectionState, {
                type: "connectionForgotten",
                baseUrl: null,
            }),
        ).toEqual({
            status: "unconnected",
            failure: null,
            forgottenBaseUrl: null,
        });
    });
});
