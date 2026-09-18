import { describe, expect, it } from "vitest";
import type { Connection } from "./model";
import { type ConnectionState, initialConnectionState, reduceConnectionState } from "./state";

const connection: Connection = {
    baseUrl: "https://gateway.example.com",
    token: "secret",
};

describe("reduceConnectionState", () => {
    it("starts unconnected without a failure", () => {
        expect(initialConnectionState).toEqual({
            status: "unconnected",
            failure: null,
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
            failure: {
                message: "previous failure",
                cause: null,
                baseUrl: null,
            },
        };

        expect(
            reduceConnectionState(stale, {
                type: "storedConnectionLoaded",
                connection: null,
            }),
        ).toEqual({
            status: "unconnected",
            failure: null,
        });
    });

    it("submits a candidate only from the unconnected state, clearing any failure", () => {
        const failed: ConnectionState = {
            status: "unconnected",
            failure: {
                message: "bad token",
                cause: null,
                baseUrl: null,
            },
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

    it("rejects into an unconnected state carrying the failure whole", () => {
        const probeFailure = new TypeError("Failed to fetch");
        const attempted = "https://gateway.example.com";

        expect(
            reduceConnectionState(
                {
                    status: "verifying",
                },
                {
                    type: "connectionRejected",
                    failure: {
                        message: "Network request failed: Failed to fetch",
                        cause: probeFailure,
                        baseUrl: attempted,
                    },
                },
            ),
        ).toEqual({
            status: "unconnected",
            failure: {
                message: "Network request failed: Failed to fetch",
                cause: probeFailure,
                baseUrl: attempted,
            },
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
                    failure: {
                        message: "invalid management token",
                        cause: null,
                        baseUrl: null,
                    },
                },
            ),
        ).toEqual({
            status: "unconnected",
            failure: {
                message: "invalid management token",
                cause: null,
                baseUrl: null,
            },
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
            failure: null,
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
            failure: null,
        });

        expect(
            reduceConnectionState(
                {
                    status: "unconnected",
                    failure: {
                        message: "boom",
                        cause: null,
                        baseUrl: null,
                    },
                },
                forgetting,
            ),
        ).toEqual({
            status: "unconnected",
            failure: null,
        });
    });
});
