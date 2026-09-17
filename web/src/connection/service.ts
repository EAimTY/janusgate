import { type Accessor, createSignal } from "solid-js";
import {
    type Connection,
    clearStoredConnection,
    readStoredConnection,
    writeStoredConnection,
} from "/src/connection/model";
import {
    type ConnectionEvent,
    type ConnectionState,
    initialConnectionState,
    reduceConnectionState,
} from "/src/connection/state";
import { describeManagementFailure } from "/src/management-api/errors";

export type ConnectionServiceDependencies = {
    readonly storage: Storage | null;
    /** The connection probe: `providers:list`, the first call of §3.1. */
    readonly probe: (connection: Connection) => Promise<void>;
};

export type ConnectionService = {
    readonly connectionState: Accessor<ConnectionState>;
    /** The connection to administer with, or `null` while unconnected. */
    readonly currentConnection: () => Connection | null;
    readonly connect: (candidate: Connection) => Promise<void>;
    readonly forget: () => void;
    /** A management call answered 401: the pair is cleared, setup explains it. */
    readonly reportUnauthorized: (notice: string) => void;
};

/**
 * Owns the connection pair's lifecycle: it resumes what storage holds, probes
 * and persists new candidates, and clears the pair whenever authorization is
 * proven dead.
 */
export function createConnectionService(
    dependencies: ConnectionServiceDependencies,
): ConnectionService {
    let currentState = reduceConnectionState(initialConnectionState, {
        type: "storedConnectionLoaded",
        connection: readStoredConnection(dependencies.storage),
    });
    const [connectionState, setConnectionState] = createSignal<ConnectionState>(currentState);

    // Reducer state is the source of truth and is updated synchronously; the
    // signal mirrors it for rendering, whose reads commit with the next flush.
    // Guards must never read through the mirror.
    const dispatch = (event: ConnectionEvent): void => {
        currentState = reduceConnectionState(currentState, event);
        setConnectionState(currentState);
    };

    const currentConnection = (): Connection | null => {
        return currentState.status === "connected" ? currentState.connection : null;
    };

    const connect = async (candidate: Connection): Promise<void> => {
        // A probe is only meaningful without a connection; a second submission
        // while one is pending would race two candidates against one slot.
        if (currentState.status !== "unconnected") {
            return;
        }

        dispatch({
            type: "connectionSubmitted",
        });

        try {
            await dependencies.probe(candidate);
        } catch (error) {
            dispatch({
                type: "connectionRejected",
                notice: describeManagementFailure(error),
            });
            return;
        }

        writeStoredConnection(dependencies.storage, candidate);
        dispatch({
            type: "connectionAccepted",
            connection: candidate,
        });
    };

    const forget = (): void => {
        clearStoredConnection(dependencies.storage);
        dispatch({
            type: "connectionForgotten",
        });
    };

    const reportUnauthorized = (notice: string): void => {
        clearStoredConnection(dependencies.storage);
        dispatch({
            type: "connectionRejected",
            notice,
        });
    };

    return {
        connectionState,
        currentConnection,
        connect,
        forget,
        reportUnauthorized,
    };
}
