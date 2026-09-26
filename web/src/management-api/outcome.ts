import { isAbortFailure } from "/src/management-api/errors";

/**
 * How one management call ended, as a screen consumes it: a value, a failure
 * carried whole as its cause — the diagnosis, not the outcome, makes both the
 * message and the errno from the cause, so a transport failure can never be
 * rendered as if the gateway had worded it — or nothing to do because the
 * visitor navigated away mid-flight.
 */
export type ManagementCallOutcome<TResult> =
    | {
          readonly status: "ok";
          readonly result: TResult;
      }
    | {
          readonly status: "failed";
          readonly cause: unknown;
      }
    | {
          readonly status: "aborted";
      };

export async function performManagementCall<TResult>(
    operation: () => Promise<TResult>,
): Promise<ManagementCallOutcome<TResult>> {
    try {
        return {
            status: "ok",
            result: await operation(),
        };
    } catch (error) {
        if (isAbortFailure(error)) {
            return {
                status: "aborted",
            };
        }

        return {
            status: "failed",
            cause: error,
        };
    }
}
