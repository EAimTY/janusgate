import type { Connection } from "/src/connection/model";

/**
 * The lifecycle of the connection pair (README §3.1). State is a value; events
 * are the only way it changes, and every transition is stated explicitly.
 */
export type ConnectionState =
    | {
          readonly status: "unconnected";
          readonly notice: string | null;
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
          readonly notice: string;
      }
    | {
          readonly type: "connectionForgotten";
      };

export const initialConnectionState: ConnectionState = {
    status: "unconnected",
    notice: null,
};

/**
 * Applies one lifecycle event. Events that do not apply to the current state
 * leave it untouched: a submission is only meaningful without a connection, an
 * acceptance only while verifying, and a rejection (a failed probe or a 401
 * answering a management call) always lands on setup with the gateway's
 * message.
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
                      notice: null,
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
                notice: event.notice,
            };

        case "connectionForgotten":
            return {
                status: "unconnected",
                notice: null,
            };
    }
}
