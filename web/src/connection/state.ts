import type { Connection } from "/src/connection/model";

/**
 * A refused connection attempt, kept whole so the setup screen can explain it
 * (README §3.1): the message is what a person reads, `cause` is what the
 * diagnosis needs, and `baseUrl` is the address the failed attempt carried, so
 * the reconnect screen seeds its address field with it and a later edit of the
 * field cannot misattribute the failure. No token travels with it: the token
 * is a bearer secret the user must re-enter, and its loss is why the reconnect
 * exists.
 */
type ConnectionFailure = {
    readonly message: string;
    readonly cause: unknown;
    readonly baseUrl: string | null;
};

/**
 * The lifecycle of the connection pair (README §3.1). State is a value; events
 * are the only way it changes, and every transition is stated explicitly.
 */
export type ConnectionState =
    | {
          readonly status: "unconnected";
          readonly failure: ConnectionFailure | null;
          /**
           * The address the connection the operator deliberately forgot was
           * serving, so the setup form reopens with it and the manual token
           * rotation never demands an origin retype. A refused attempt's
           * address travels on its failure instead, so the two sources never
           * both carry a seed.
           */
          readonly forgottenBaseUrl: string | null;
      }
    | {
          readonly status: "verifying";
      }
    | {
          readonly status: "connected";
          readonly connection: Connection;
      };

export type ConnectionEvent =
    | {
          readonly type: "storedConnectionLoaded";
          readonly connection: Connection | null;
      }
    | {
          readonly type: "connectionSubmitted";
      }
    | {
          readonly type: "connectionAccepted";
          readonly connection: Connection;
      }
    | {
          readonly type: "connectionRejected";
          readonly failure: ConnectionFailure;
      }
    | {
          readonly type: "connectionForgotten";
          /**
           * The forgotten connection's address, captured by the service
           * before the pair drops — `null` when nothing was connected.
           */
          readonly baseUrl: string | null;
      };

export const initialConnectionState: ConnectionState = {
    status: "unconnected",
    failure: null,
    forgottenBaseUrl: null,
};

/**
 * Applies one lifecycle event. Events that do not apply to the current state
 * leave it untouched: a submission is only meaningful without a connection, an
 * acceptance only while verifying, and a rejection (a failed probe or a 401
 * answering a management call) always lands on setup with the gateway's
 * failure.
 */
export function reduceConnectionState(
    state: ConnectionState,
    event: ConnectionEvent,
): ConnectionState {
    switch (event.type) {
        case "storedConnectionLoaded":
            return event.connection === null
                ? {
                      status: "unconnected",
                      failure: null,
                      forgottenBaseUrl: null,
                  }
                : {
                      status: "connected",
                      connection: event.connection,
                  };

        case "connectionSubmitted":
            return state.status === "unconnected"
                ? {
                      status: "verifying",
                  }
                : state;

        case "connectionAccepted":
            return state.status === "verifying"
                ? {
                      status: "connected",
                      connection: event.connection,
                  }
                : state;

        case "connectionRejected":
            return {
                status: "unconnected",
                failure: event.failure,
                forgottenBaseUrl: null,
            };

        case "connectionForgotten":
            return {
                status: "unconnected",
                failure: null,
                forgottenBaseUrl: event.baseUrl,
            };
    }
}
