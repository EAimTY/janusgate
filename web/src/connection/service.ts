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
import { describeManagementFailure, ManagementApiError } from "/src/management-api/errors";
import { ErrorCode } from "/src/management-api/generated/v0";

type ConnectionServiceDependencies = {
    readonly storage: Storage | null;
    /**
     * The connection probe: `providers:list`, the first call of §3.1. The
     * signal is the attempt's cancel channel, owned by the service: an
     * aborted attempt is the operator's cancellation, never a refusal — the
     * probe's own deadline and failures still surface as rejections.
     */
    readonly probe: (connection: Connection, signal: AbortSignal) => Promise<void>;
};

type ConnectionService = {
    readonly connectionState: Accessor<ConnectionState>;
    /** The connection to administer with, or `null` while unconnected. */
    readonly currentConnection: () => Connection | null;
    readonly connect: (candidate: Connection) => Promise<void>;
    /**
     * Cancels the connect probe in flight, if one is: the attempt ends and
     * the machine records a cancellation — control flow, never a refusal
     * (README §3.1). A probe that already answered is untouched.
     */
    readonly cancelConnect: () => void;
    /**
     * Drops the pair at the operator's hand: storage is cleared and setup
     * reopens with the address the forgotten connection served.
     */
    readonly forget: () => void;
    /**
     * A management call answered 401: the pair is cleared, setup explains it,
     * and the reconnect screen reopens seeded with the dying address.
     */
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

    // The cancel handle of the attempt in flight, or `null` between attempts.
    // Only one probe runs at a time (the connect guard), so the handle cannot
    // be raced by a second submission.
    let connectController: AbortController | null = null;

    const connect = async (candidate: Connection): Promise<void> => {
        // A probe is only meaningful without a connection; a second submission
        // while one is pending would race two candidates against one slot.
        if (currentState.status !== "unconnected") {
            return;
        }

        const controller = new AbortController();
        connectController = controller;
        dispatch({
            type: "connectionSubmitted",
        });

        try {
            await dependencies.probe(candidate, controller.signal);
        } catch (error) {
            // The service is the only aborter of this signal, so an aborted
            // attempt is the operator's cancellation: control flow, never a
            // refusal — the machine records no diagnosis and no failure
            // seeds the setup form. The probe's own deadline and failures
            // arrive with the signal clean and reject as every refusal does.
            if (controller.signal.aborted) {
                dispatch({
                    type: "connectionCancelled",
                });
                return;
            }

            dispatch({
                type: "connectionRejected",
                failure: {
                    message: describeManagementFailure(error),
                    cause: error,
                    baseUrl: candidate.baseUrl,
                },
            });
            return;
        } finally {
            if (connectController === controller) {
                connectController = null;
            }
        }

        // A transport that cannot abort can still answer through the cancel:
        // the operator's word stands, and the cancelled attempt is never
        // accepted — its landing is the cancellation, whatever answered.
        if (controller.signal.aborted) {
            dispatch({
                type: "connectionCancelled",
            });
            return;
        }

        writeStoredConnection(dependencies.storage, candidate);

        dispatch({
            type: "connectionAccepted",
            connection: candidate,
        });
    };

    const cancelConnect = (): void => {
        connectController?.abort();
    };

    const forget = (): void => {
        // The address is captured before the pair drops, so the setup form
        // reopens with it — the standard manual token rotation (disconnect,
        // forget, paste, connect) never demands an origin retype. The token
        // deliberately does not travel: it is a bearer secret only the user
        // can re-enter.
        const forgottenBaseUrl =
            currentState.status === "connected" ? currentState.connection.baseUrl : null;

        clearStoredConnection(dependencies.storage);

        dispatch({
            type: "connectionForgotten",
            baseUrl: forgottenBaseUrl,
        });
    };

    const reportUnauthorized = (notice: string): void => {
        // The 401 answers the connection in charge (the caller guards that),
        // so the address is captured before the rejection drops the pair: the
        // reconnect screen reopens with the field seeded from it. The token
        // deliberately does not travel — it is a bearer secret only the user
        // can re-enter, and its loss is why the reconnect exists.
        const dyingBaseUrl =
            currentState.status === "connected" ? currentState.connection.baseUrl : null;

        clearStoredConnection(dependencies.storage);

        dispatch({
            type: "connectionRejected",
            failure: {
                message: notice,
                // The only failure this path sees is the version's 401, so the
                // cause carries that category rather than nothing: the panel's
                // diagnosis can then read it like any other rejection.
                cause: new ManagementApiError("gateway", notice, {
                    errno: ErrorCode.Unauthorized,
                    httpStatus: 401,
                }),
                baseUrl: dyingBaseUrl,
            },
        });
    };

    return {
        connectionState,
        currentConnection,
        connect,
        cancelConnect,
        forget,
        reportUnauthorized,
    };
}
