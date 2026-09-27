import { isAbortFailure, ManagementApiError } from "/src/management-api/errors";
import { ErrorCode } from "/src/management-api/generated/v0";

/**
 * Whether the browser reached the instance's origin while the failed call was
 * being diagnosed:
 *
 * - `reached` — a cross-origin no-CORS request to the origin was answered, so
 *   the network path works and a blocked fetch is the browser's CORS layer.
 * - `unreached` — that request failed too, so nothing answered.
 * - `unknown` — no probe ran, or it was cancelled or timed out; a deadline is
 *   never read as an answer about the network.
 */
export type Reachability = "reached" | "unreached" | "unknown";

/** The management action whose failure is being explained. */
export type ActionName =
    | "connect"
    | "ensure-definition"
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
 * of one — where one failure hides two possible causes, it states each
 * cause's recovery conditionally instead of claiming either.
 */
export type Diagnosis = {
    readonly kind:
        | "blocked"
        | "mixed-content"
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

/**
 * Whether the page and the address form the one pairing the browser refuses
 * outright: an HTTPS page may not call an HTTP address, and it blocks every
 * such call deterministically before any request is sent — so no probe could
 * observe anything and the cause is certain without one.
 *
 * Calls to a loopback address are exempt: browsers deliver those from an
 * HTTPS page normally (they are potentially trustworthy), so their failure is
 * never a mixed-content block.
 */
function isMixedContent(pageOrigin: string, baseUrl: string): boolean {
    try {
        if (new URL(pageOrigin).protocol !== "https:" || new URL(baseUrl).protocol !== "http:") {
            return false;
        }

        // WHATWG leaves a single trailing dot on a hostname (`localhost.`),
        // though the browser resolves it like the bare name, so strip it
        // before the loopback comparisons; over-exemption is always safe,
        // because the probe then decides.
        const host = new URL(baseUrl).hostname.replace(/\.$/, "");

        return !(
            host === "localhost" ||
            host.endsWith(".localhost") ||
            // The URL parser normalizes IPv4 hosts to dotted-quad decimal, so
            // the whole 127.0.0.0/8 loopback is exactly this shape — four
            // dotted decimals, and no domain that merely borrows the prefix.
            /^127(\.\d{1,3}){3}$/.test(host) ||
            host === "0.0.0.0" ||
            host === "[::1]"
        );
    } catch {
        // An unparsable address is the connection screen's problem to state;
        // mixed content is not the explanation to offer for it.
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

        // The definition write's collision is not the operator's input: its
        // name is the preset's id and its base version is the stored latest,
        // so the only recovery is picking the moved version back up — the
        // shell's refresh re-runs the boot read that re-binds it, keeping the
        // in-progress form intact.
        case "ensure-definition":
            return "The stored version moved — use Refresh in the bar above; it re-reads the stored versions and leaves this form untouched — then submit again.";

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
        // The gateway fails a discovery with the same upstream error whether
        // the call itself was refused or the upstream answered 200 with an
        // empty model list — a fresh pre-onboarded account does exactly that
        // (root README §7.1) — so the fix names both recoveries the
        // provider's kind admits (an OAuth account reconnects, a static key
        // is corrected in place) as conditionals and claims neither cause.
        if (isOAuthProvider !== undefined) {
            return isOAuthProvider
                ? "If the provider's authorization lapsed, reconnect it on the provider's edit page; if the upstream genuinely lists no models yet, add one by hand from this page."
                : "If the provider's base URL or API key is wrong, correct it on the provider's edit page; if the upstream genuinely lists no models yet, add one by hand from this page.";
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

    // The one refusal the diagnosis can be certain of without evidence: the
    // browser blocks this pairing before any request is sent, so the probe is
    // moot and every reachability reading of it would be a guess. The kind
    // keys the connection screen's probing: anything but `no-answer` issues
    // no request.
    if (isMixedContent(pageOrigin, baseUrl)) {
        return {
            kind: "mixed-content",
            title: `An HTTPS page cannot call the HTTP address ${baseUrl}.`,
            detail: "This page is served over HTTPS, and the browser blocks its calls to HTTP addresses outright, so the gateway was never contacted.",
            fix: "Serve this app over HTTP as well, or add a `tls` section to that instance's configuration file so the gateway answers HTTPS.",
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
