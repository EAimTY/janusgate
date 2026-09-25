import { isAbortFailure, ManagementApiError } from "/src/management-api/errors";
import { ErrorCode } from "/src/management-api/generated/v0";

/**
 * Whether the browser reached the instance's origin while the failed call was
 * being diagnosed:
 *
 * - `reached` — a cross-origin no-CORS request to the origin was answered, so
 *   the network path works and a blocked fetch is the browser's CORS layer.
 * - `unreached` — that request failed too, so nothing answered.
 * - `unknown` — no probe ran, or it was cancelled.
 */
export type Reachability = "reached" | "unreached" | "unknown";

/** The management action whose failure is being explained. */
export type ActionName =
    | "connect"
    | "create-provider"
    | "update-provider"
    | "delete-provider"
    | "discover-models"
    | "create-model"
    | "update-model"
    | "delete-model"
    | "list-usage"
    | "query-usage"
    | "create-api-key"
    | "update-api-key"
    | "delete-api-key"
    | "start-oauth"
    | "complete-oauth"
    | "reauthorize";

export type ActionContext = {
    /** The connection's base URL, as the operator entered it. */
    readonly baseUrl: string;
    /** The origin this app is served from — the value a CORS allowlist needs. */
    readonly pageOrigin: string;
    readonly action?: ActionName | undefined;
    /**
     * Whether the provider the action addressed logs in upstream, where one
     * is known: the recovery a discovery failure names depends on whether the
     * provider reconnects (OAuth) or holds a static key corrected in place.
     */
    readonly isOAuthProvider?: boolean | undefined;
    readonly reachability?: Reachability;
};

/**
 * What the panel says about a failure. The kind selects the panel's own
 * explanation; `detail` always carries the gateway's message when the gateway
 * rendered one, and `fix` names a recovery only where the app can be certain
 * of one.
 */
export type Diagnosis = {
    readonly kind:
        | "blocked"
        | "unreachable"
        | "no-answer"
        | "unauthorized"
        | "gone"
        | "conflict"
        | "upstream"
        | "internal"
        | "rejected"
        | "contract"
        | "gateway"
        | "unknown";
    readonly title: string;
    readonly detail: string;
    readonly fix: string | null;
};

const CORS_SETTING = "management_api.cors.allowed_origin";

function isCrossOrigin(baseUrl: string, pageOrigin: string): boolean {
    try {
        return new URL(baseUrl).origin !== new URL(pageOrigin).origin;
    } catch {
        // An unparsable address is the connection screen's problem to state;
        // CORS is not the explanation to offer for it.
        return false;
    }
}

function conflictFix(action: ActionName | undefined): string {
    switch (action) {
        case "create-model":
        case "update-model":
            return "Another model already routes that public ID.";

        case "create-api-key":
        case "update-api-key":
            return "Another key already holds that secret; generate a fresh one.";

        case "create-provider":
        case "update-provider":
        case "complete-oauth":
            return "Choose a provider name no other row holds; a changed model prefix can also collide with an existing model ID.";

        default:
            return "Choose an identifier no other row holds.";
    }
}

function upstreamFix(context: ActionContext): string {
    const { action, isOAuthProvider } = context;

    if (action === "discover-models") {
        // The recovery a discovery refusal names depends on how the provider
        // authenticates: an OAuth account reconnects, a static key is
        // corrected in place.
        if (isOAuthProvider !== undefined) {
            return isOAuthProvider
                ? "The provider's authorization expired; reconnect it on the provider's edit page."
                : "Check the provider's base URL and API key on its edit page.";
        }

        return "Check the provider's state and credentials.";
    }

    switch (action) {
        case "start-oauth":
        case "complete-oauth":
        case "reauthorize":
            return "The provider refused the exchange; start the flow again from this page.";

        default:
            return "Check the provider's credentials and reachability.";
    }
}

function networkDiagnosis(context: ActionContext): Diagnosis {
    const { baseUrl, pageOrigin, reachability = "unknown" } = context;
    const origin = originOf(pageOrigin);

    if (!isCrossOrigin(baseUrl, pageOrigin)) {
        return {
            kind: "unreachable",
            title: `Nothing answered at ${baseUrl}.`,
            detail: "The browser could not complete the connection.",
            fix: "Check the address, that the gateway is running, and that its `listen` address is reachable from this browser.",
        };
    }

    switch (reachability) {
        case "reached":
            return {
                kind: "blocked",
                title: `The gateway answered, but the browser blocked the response from ${baseUrl}.`,
                detail: "The browser withheld the response from this page.",
                fix: `Add ${origin} to \`${CORS_SETTING}\` in that instance's configuration file and restart the gateway. Serving this app from the gateway's own origin avoids the allowlist entirely.`,
            };

        case "unreached":
            return {
                kind: "unreachable",
                title: `Nothing answered at ${baseUrl}.`,
                detail: "The browser could not complete the connection.",
                fix: "Check the address, that the gateway is running, and that it is reachable from this browser over this scheme.",
            };

        case "unknown":
            return {
                kind: "no-answer",
                title: `No answer from ${baseUrl}.`,
                detail: "The browser received no answer it could read.",
                fix: `Either nothing is listening at that address, or the browser blocked the response before this page could read it — add ${origin} to \`${CORS_SETTING}\` and restart the gateway.`,
            };
    }
}

function originOf(value: string): string {
    try {
        return new URL(value).origin;
    } catch {
        return value;
    }
}

function fromGatewayError(error: ManagementApiError, context: ActionContext): Diagnosis {
    const message = error.message;

    switch (error.errno) {
        case ErrorCode.Unauthorized:
            return {
                kind: "unauthorized",
                title: "The gateway rejected that management token.",
                detail: message,
                fix: "The token must match `management_api.token` in that instance's configuration file; the gateway reads it live, so a changed token needs no restart.",
            };

        case ErrorCode.NotFound:
            return {
                kind: "gone",
                title: "The gateway holds no such row.",
                detail: message,
                fix: null,
            };

        case ErrorCode.Conflict:
            return {
                kind: "conflict",
                title: "The gateway refused: an identifier is already taken.",
                detail: message,
                fix: conflictFix(context.action),
            };

        case ErrorCode.UpstreamError:
            return {
                kind: "upstream",
                title: "The upstream provider refused the call or was unreachable.",
                detail: message,
                fix: upstreamFix(context),
            };

        case ErrorCode.Internal:
            return {
                kind: "internal",
                title: "The gateway failed internally.",
                detail: message,
                fix: "The cause is in the gateway's stderr log; internal causes are never returned to this page.",
            };

        default:
            return {
                kind: "rejected",
                title: "The gateway rejected the request.",
                detail: message,
                fix: null,
            };
    }
}

/**
 * Explains a failed management call in the panel's own words, keeping the
 * gateway's message verbatim as the detail wherever the gateway rendered one
 * (README §4.3): the app never replaces what the gateway said, it only says
 * what the operator can do about it — and only where it can be certain. A
 * call the browser refused never reached the gateway, so its diagnosis
 * speaks for the browser instead.
 */
export function diagnoseFailure(error: unknown, context: ActionContext): Diagnosis {
    if (error instanceof ManagementApiError) {
        if (error.category === "contract") {
            return {
                kind: "contract",
                title: "The gateway answered with a shape this build does not know.",
                detail: error.message,
                fix: "This build's generated bindings and the gateway's management API disagree; regenerate the bindings or rebuild this frontend from a matching revision.",
            };
        }

        if (error.category === "gateway") {
            return fromGatewayError(error, context);
        }

        if (error.httpStatus === null) {
            // The browser refused the call before any answer existed, so
            // there is no gateway message to keep: the network diagnosis's
            // own human wording is the detail (README §6.4).
            return networkDiagnosis(context);
        }

        return {
            kind: "gateway",
            title: `The gateway answered HTTP ${error.httpStatus} without a management error body.`,
            detail: error.message,
            fix: "Check the gateway's stderr log for the cause.",
        };
    }

    return {
        kind: "unknown",
        title: "The call failed.",
        detail: error instanceof Error ? error.message : String(error),
        fix: null,
    };
}

/**
 * Asks whether the browser can reach an instance's origin at all, using a
 * no-CORS request: the browser resolves it with an opaque response whenever
 * the server answered, regardless of its CORS headers, so a resolved probe
 * proves the network path while a rejected one proves nothing answered.
 *
 * The probe never carries the management token and never reads a body.
 */
export async function probeReachability(
    baseUrl: string,
    signal?: AbortSignal,
): Promise<Reachability> {
    try {
        await fetch(`${baseUrl}/`, {
            method: "GET",
            mode: "no-cors",
            cache: "no-store",
            credentials: "omit",
            signal: signal ?? null,
        });

        return "reached";
    } catch (error) {
        if (isAbortFailure(error)) {
            return "unknown";
        }

        return error instanceof TypeError ? "unreached" : "unknown";
    }
}
