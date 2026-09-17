import { describe, expect, it } from "vitest";
import type { Connection } from "./model";
import { type ConnectionState, initialConnectionState, reduceConnectionState } from "./state";

const connection: Connection = {
    baseUrl: "https://gateway.example.com",
    token: "secret",
};

describe("reduceConnectionState", () => {
    it("starts unconnected without a notice", () => {
        expect(initialConnectionState).toEqual({
            status: "unconnected",
            notice: null,
        });
    });

    it("loads a stored connection into the connected state", () => {
        expect(
            reduceConnectionState(initialConnectionState, {
                type: "storedConnectionLoaded",
                connection,
            }),
        ).toEqual({
            status: "connected",
            connection,
        });
    });

    it("loads absent storage into a clean unconnected state", () => {
        const stale: ConnectionState = {
            status: "unconnected",
            notice: "previous failure",
        };

        expect(
            reduceConnectionState(stale, {
                type: "storedConnectionLoaded",
                connection: null,
            }),
        ).toEqual({
            status: "unconnected",
            notice: null,
        });
    });

    it("submits a candidate only from the unconnected state, clearing any notice", () => {
        const failed: ConnectionState = {
            status: "unconnected",
            notice: "bad token",
        };

        expect(
            reduceConnectionState(failed, {
                type: "connectionSubmitted",
            }),
        ).toEqual({
            status: "verifying",
        });

        expect(
            reduceConnectionState(initialConnectionState, {
                type: "connectionSubmitted",
            }),
        ).toEqual({
            status: "verifying",
        });
    });

    it("ignores a submission while verifying or connected", () => {
        const verifying: ConnectionState = {
            status: "verifying",
        };

        expect(
            reduceConnectionState(verifying, {
                type: "connectionSubmitted",
            }),
        ).toBe(verifying);

        const connected: ConnectionState = {
            status: "connected",
            connection,
        };

        expect(
            reduceConnectionState(connected, {
                type: "connectionSubmitted",
            }),
        ).toBe(connected);
    });

    it("accepts a candidate only while verifying", () => {
        expect(
            reduceConnectionState(
                {
                    status: "verifying",
                },
                {
                    type: "connectionAccepted",
                    connection,
                },
            ),
        ).toEqual({
            status: "connected",
            connection,
        });

        expect(
            reduceConnectionState(initialConnectionState, {
                type: "connectionAccepted",
                connection,
            }),
        ).toBe(initialConnectionState);

        expect(
            reduceConnectionState(
                {
                    status: "connected",
                    connection,
                },
                {
                    type: "connectionAccepted",
                    connection: {
                        baseUrl: "https://other.example.com",
                        token: "other",
                    },
                },
            ),
        ).toEqual({
            status: "connected",
            connection,
        });
    });

    it("rejects into an unconnected state carrying the gateway's message", () => {
        expect(
            reduceConnectionState(
                {
                    status: "verifying",
                },
                {
                    type: "connectionRejected",
                    notice: "HTTP 401",
                },
            ),
        ).toEqual({
            status: "unconnected",
            notice: "HTTP 401",
        });

        // The lazy revalidation path: a 401 answering a management call while connected.
        expect(
            reduceConnectionState(
                {
                    status: "connected",
                    connection,
                },
                {
                    type: "connectionRejected",
                    notice: "invalid management token",
                },
            ),
        ).toEqual({
            status: "unconnected",
            notice: "invalid management token",
        });
    });

    it("forgets back to a clean unconnected state from any state", () => {
        const forgetting = {
            type: "connectionForgotten",
        } as const;

        expect(
            reduceConnectionState(
                {
                    status: "connected",
                    connection,
                },
                forgetting,
            ),
        ).toEqual({
            status: "unconnected",
            notice: null,
        });

        expect(
            reduceConnectionState(
                {
                    status: "verifying",
                },
                forgetting,
            ),
        ).toEqual({
            status: "unconnected",
            notice: null,
        });

        expect(
            reduceConnectionState(
                {
                    status: "unconnected",
                    notice: "boom",
                },
                forgetting,
            ),
        ).toEqual({
            status: "unconnected",
            notice: null,
        });
    });
});
