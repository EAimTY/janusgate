import { ErrorCode } from "/src/management-api/generated/v0";

/**
 * How a management call failed:
 *
 * - `gateway` — the gateway rendered its errno-shaped error body. Its `message`
 *   is the only message shown to the administrator (README §4.3).
 * - `transport` — no management error body was available: a network failure, a
 *   non-JSON body, or an HTTP status the gateway did not shape. Only the status
 *   is reportable.
 * - `contract` — a 2xx body that does not match the expected shape.
 */
export type ManagementFailureCategory = "gateway" | "transport" | "contract";

export type ManagementApiErrorDetails = {
    readonly httpStatus?: number | null;
    readonly errno?: ErrorCode | null;
    readonly cause?: unknown;
};

/** Every failure a management call can end in. */
export class ManagementApiError extends Error {
    readonly category: ManagementFailureCategory;
    readonly httpStatus: number | null;
    readonly errno: ErrorCode | null;

    constructor(
        category: ManagementFailureCategory,
        message: string,
        details: ManagementApiErrorDetails = {},
    ) {
        super(message, {
            cause: details.cause,
        });

        this.name = "ManagementApiError";
        this.category = category;
        this.httpStatus = details.httpStatus ?? null;
        this.errno = details.errno ?? null;
    }
}

/**
 * Thrown by response validation when a 2xx body does not match the shape the
 * generated bindings declare. The client turns it into a `ManagementApiError`.
 */
export class ContractViolationError extends Error {
    constructor(message: string) {
        super(message);
        this.name = "ContractViolationError";
    }
}

/** A 401 carrying the version's `unauthorized` errno proves the token dead. */
export function isUnauthorizedFailure(value: unknown): boolean {
    return value instanceof ManagementApiError && value.errno === ErrorCode.Unauthorized;
}

/** Aborts are control flow, never a failure to render. */
export function isAbortFailure(value: unknown): boolean {
    return (
        typeof value === "object" && value !== null && Reflect.get(value, "name") === "AbortError"
    );
}

export function managementFailureErrno(value: unknown): ErrorCode | null {
    return value instanceof ManagementApiError ? value.errno : null;
}

/**
 * The administrator-facing message of a failure: the gateway's own message for
 * a rendered management error, and the underlying error's message when the
 * gateway supplied none (a network failure, for instance).
 */
export function describeManagementFailure(value: unknown): string {
    if (value instanceof Error) {
        return value.message;
    }

    return String(value);
}
