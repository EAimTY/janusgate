import type { JSX } from "@solidjs/web";
import { translation } from "/src/i18n";
import { corsRecoveryMessage, networkTitleMessage } from "/src/i18n/rich";
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
type Reachability = "reached" | "unreached" | "unknown";

/** The management action whose failure is being explained. */
export type ActionName =
    | "connect"
    | "create-pool"
    | "update-pool"
    | "delete-pool"
    | "create-route"
    | "update-route"
    | "delete-route"
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

type ActionContext = {
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
    readonly context: ActionContext;
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
    readonly title: JSX.Element;
    readonly detail: string;
    readonly fix: JSX.Element;
    readonly rawDetail?: string;
};

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
            return translation()(
                ($) =>
                    $.diagnosis
                        .thisAccountAlreadyHoldsThatExactUpstreamModelOpenItsExistingInventoryRow,
            );

        case "create-route":
        case "update-route":
            return translation()(
                ($) =>
                    $.diagnosis
                        .anotherPublicRouteHoldsThatPublicIDChooseAnExplicitDifferentNameOrEdit,
            );
        case "create-pool":
        case "update-pool":
            return translation()(
                ($) => $.diagnosis.anotherPoolHoldsThatNameChooseADifferentPoolName,
            );
        case "delete-pool":
            return translation()(
                ($) =>
                    $.diagnosis
                        .publicRoutesStillReferenceThisPoolChangeOrDeleteThoseRoutesBeforeDeletingThe,
            );

        case "create-api-key":
        case "update-api-key":
            return translation()(
                ($) => $.diagnosis.anotherKeyAlreadyHoldsThatSecretGenerateAFreshOne,
            );

        // The definition write's collision is not the operator's input: its
        // name is the preset's id and its base version is the stored latest,
        // so the only recovery is picking the moved version back up — the
        // shell's refresh re-runs the boot read that re-binds it, keeping the
        // in-progress form intact.
        case "ensure-definition":
            return translation()(
                ($) => $.diagnosis.theStoredVersionMovedUseRefreshInTheBarAboveItRereadsTheStored,
            );

        case "create-provider":
        case "update-provider":
            return translation()(($) => $.diagnosis.chooseAProviderNameNoOtherRowHolds);

        // A creation flow's completion conflicts only on the name the flow
        // was started under, and the gateway consumes the flow on the
        // completion attempt even when it answers that conflict (root README
        // §7.1): the page keeps the flow live — and with it the name locked — while the state is already spent, so no field a
        // rename could point at is editable. The one in-page escape is the
        // step's own "Start over" control, which ends the flow and unlocks
        // the name for the fresh creation's rename. A reauthorize
        // completion can never answer this conflict: it replaces an existing
        // row's credentials and touches no unique column, so it stays on the
        // shared arms' behalf alone.
        case "complete-oauth":
            return translation()(
                ($) =>
                    $.diagnosis
                        .anotherProviderTookThisNameBeforeTheFlowCouldFinishStartOverEndsThis,
            );

        default:
            return translation()(($) => $.diagnosis.chooseAnIdentifierNoOtherRowHolds);
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
                ? translation()(
                      ($) =>
                          $.diagnosis
                              .ifTheProvidersAuthorizationLapsedReconnectItOnTheProvidersEditPageIfThe,
                  )
                : translation()(
                      ($) => $.diagnosis.ifTheProvidersBaseURLOrAPIKeyIsWrongCorrectItOnThe,
                  );
        }

        return translation()(($) => $.diagnosis.checkTheProvidersStateAndCredentials);
    }

    switch (action) {
        case "start-oauth":
        case "complete-oauth":
        case "reauthorize":
            return translation()(
                ($) => $.diagnosis.theProviderRefusedTheExchangeStartTheFlowAgainFromThisPage,
            );

        default:
            return translation()(($) => $.diagnosis.checkTheProvidersCredentialsAndReachability);
    }
}

function networkDiagnosis(context: ActionContext, rawDetail: string): Diagnosis {
    const { baseUrl, pageOrigin, reachability = "unknown" } = context;
    const origin = originOf(pageOrigin);

    if (!isCrossOrigin(baseUrl, pageOrigin)) {
        return {
            context,
            rawDetail,
            kind: "unreachable",
            get title() {
                return networkTitleMessage("unreachable", baseUrl);
            },
            get detail() {
                return translation()(($) => $.diagnosis.theBrowserCouldNotCompleteTheConnection);
            },
            get fix() {
                return translation()(
                    ($) =>
                        $.diagnosis.checkTheAddressThatTheGatewayIsRunningAndThatItsListenAddressIs,
                );
            },
        };
    }

    // The one refusal the diagnosis can be certain of without evidence: the
    // browser blocks this pairing before any request is sent, so the probe is
    // moot and every reachability reading of it would be a guess. The kind
    // keys the connection screen's probing: anything but `no-answer` issues
    // no request.
    if (isMixedContent(pageOrigin, baseUrl)) {
        return {
            context,
            rawDetail,
            kind: "mixed-content",
            get title() {
                return networkTitleMessage("mixed-content", baseUrl);
            },
            get detail() {
                return translation()(
                    ($) => $.diagnosis.thisPageIsServedOverHTTPSAndTheBrowserBlocksItsCallsToHTTP,
                );
            },
            get fix() {
                return translation()(
                    ($) => $.diagnosis.serveThisAppOverHTTPAsWellOrAddATlsSectionToThat,
                );
            },
        };
    }

    switch (reachability) {
        case "reached":
            return {
                context,
                rawDetail,
                kind: "blocked",
                get title() {
                    return networkTitleMessage("blocked", baseUrl);
                },
                get detail() {
                    return translation()(
                        ($) => $.diagnosis.theBrowserWithheldTheResponseFromThisPage,
                    );
                },
                get fix() {
                    return corsRecoveryMessage(true, origin);
                },
            };

        case "unreached":
            return {
                context,
                rawDetail,
                kind: "unreachable",
                get title() {
                    return networkTitleMessage("unreachable", baseUrl);
                },
                get detail() {
                    return translation()(
                        ($) => $.diagnosis.theBrowserCouldNotCompleteTheConnection,
                    );
                },
                get fix() {
                    return translation()(
                        ($) =>
                            $.diagnosis
                                .checkTheAddressThatTheGatewayIsRunningAndThatItIsReachableFrom,
                    );
                },
            };

        case "unknown":
            return {
                context,
                rawDetail,
                kind: "no-answer",
                get title() {
                    return networkTitleMessage("no-answer", baseUrl);
                },
                get detail() {
                    return translation()(($) => $.diagnosis.theBrowserReceivedNoAnswerItCouldRead);
                },
                get fix() {
                    return corsRecoveryMessage(false, origin);
                },
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
                context,
                kind: "unauthorized",
                get title() {
                    return translation()(($) => $.diagnosis.theGatewayRejectedThatManagementToken);
                },
                detail: message,
                get fix() {
                    return translation()(
                        ($) =>
                            $.diagnosis
                                .theTokenMustMatchManagementapitokenInThatInstancesConfigurationFileTheGatewayReadsIt,
                    );
                },
            };

        case ErrorCode.NotFound:
            return {
                context,
                kind: "gone",
                get title() {
                    return translation()(($) => $.diagnosis.theGatewayHoldsNoSuchRow);
                },
                detail: message,
                fix: null,
            };

        case ErrorCode.Conflict:
            return {
                context,
                kind: "conflict",
                get title() {
                    return translation()(
                        ($) => $.diagnosis.theGatewayRefusedAnIdentifierIsAlreadyTaken,
                    );
                },
                detail: message,
                get fix() {
                    return conflictFix(context.action);
                },
            };

        case ErrorCode.UpstreamError:
            return {
                context,
                kind: "upstream",
                get title() {
                    return translation()(
                        ($) => $.diagnosis.theUpstreamProviderRefusedTheCallOrWasUnreachable,
                    );
                },
                detail: message,
                get fix() {
                    return upstreamFix(context);
                },
            };

        case ErrorCode.Internal:
            return {
                context,
                kind: "internal",
                get title() {
                    return translation()(($) => $.diagnosis.theGatewayFailedInternally);
                },
                detail: message,
                get fix() {
                    return translation()(
                        ($) =>
                            $.diagnosis
                                .theCauseIsInTheGatewaysStderrLogInternalCausesAreNeverReturnedTo,
                    );
                },
            };

        default:
            return {
                context,
                kind: "rejected",
                get title() {
                    return translation()(($) => $.diagnosis.theGatewayRejectedTheRequest);
                },
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
                context,
                kind: "contract",
                get title() {
                    return translation()(
                        ($) => $.diagnosis.theGatewayAnsweredWithAShapeThisBuildDoesNotKnow,
                    );
                },
                detail: error.message,
                get fix() {
                    return translation()(
                        ($) =>
                            $.diagnosis
                                .thisBuildsGeneratedBindingsAndTheGatewaysManagementAPIDisagreeRegenerateTheBindingsOr,
                    );
                },
            };
        }

        if (error.category === "gateway") {
            return fromGatewayError(error, context);
        }

        if (error.httpStatus === null) {
            // The browser refused the call before any answer existed, so
            // there is no gateway message to keep: the network diagnosis's
            // own human wording is the detail (README §6.4).
            return networkDiagnosis(context, error.message);
        }

        const httpStatus = error.httpStatus;
        return {
            context,
            kind: "gateway",
            get title() {
                return translation()(
                    ($) => $.diagnosis.theGatewayAnsweredHTTPHttpStatusWithoutAManagementErrorBody,
                    {
                        httpStatus,
                    },
                );
            },
            detail: error.message,
            get fix() {
                return translation()(($) => $.diagnosis.checkTheGatewaysStderrLogForTheCause);
            },
        };
    }

    return {
        context,
        kind: "unknown",
        get title() {
            return translation()(($) => $.diagnosis.theCallFailed);
        },
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
