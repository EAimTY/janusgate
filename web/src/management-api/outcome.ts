import {
    describeManagementFailure,
    isAbortFailure,
    managementFailureErrno,
} from "/src/management-api/errors";

import type { ErrorCode } from "/src/management-api/generated/v0";

/**
 * How one management call ended, as a screen consumes it: a value, a failure —
 * its message for a person to read, its errno for the wire's own category, and
 * the cause whole, because the panel's diagnosis reads the failure rather than
 * its prose — or nothing to do because the visitor navigated away mid-flight.
 */
type ManagementCallOutcome<TResult> =
    | {
          readonly status: "ok";
          readonly result: TResult;
      }
    | {
          readonly status: "failed";
          readonly message: string;
          readonly errno: ErrorCode | null;
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
            message: describeManagementFailure(error),
            errno: managementFailureErrno(error),
            cause: error,
        };
    }
}
