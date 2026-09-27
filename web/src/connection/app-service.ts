import type { Connection } from "/src/connection/model";
import { createConnectionService } from "/src/connection/service";
import { createManagementApi } from "/src/management-api/client";

/** A `Storage` implementation, or `null` when the environment offers none. */
function isStorage(value: unknown): value is Storage {
    return (
        typeof value === "object" &&
        value !== null &&
        typeof Reflect.get(value, "getItem") === "function" &&
        typeof Reflect.get(value, "setItem") === "function" &&
        typeof Reflect.get(value, "removeItem") === "function"
    );
}

function browserStorage(): Storage | null {
    if (typeof window === "undefined") {
        return null;
    }

    try {
        // Storage can be absent or denied outright (privacy modes); the app
        // then lives in memory for this session only.
        const storage: unknown = window.localStorage;
        return isStorage(storage) ? storage : null;
    } catch {
        return null;
    }
}

/** How long the connection probe may run before it counts as failed. */
export const PROBE_TIMEOUT_MS = 15_000;

/**
 * The connection probe the operator cancelled. The service records it like
 * any probe outcome, but it is control flow, never a refusal (README §6.4):
 * the setup screen recognizes it and reopens the form pristine instead of
 * seeding a failure no gateway produced.
 */
export class ConnectionProbeCancelled extends Error {
    constructor() {
        super("The connection probe was cancelled.");
        this.name = "ConnectionProbeCancelled";
    }
}

/** The cancel handle of the probe in flight, or `null` between probes. */
let inFlightProbe: AbortController | null = null;

/** Whether the operator cancelled the probe now in flight. */
let isProbeCancelled = false;

/**
 * Aborts the connection probe in flight, if one is: the attempt ends and the
 * setup form reopens pristine. A probe that already answered is untouched.
 */
export function cancelConnectionProbe(): void {
    if (inFlightProbe !== null) {
        isProbeCancelled = true;
        inFlightProbe.abort();
    }
}

/**
 * The application's single connection service, wired to the browser's storage
 * and to the connection probe — `providers:list`, the first call of README
 * §3.1.
 */
export const connectionService = createConnectionService({
    storage: browserStorage(),
    probe: async (connection: Connection) => {
        const controller = new AbortController();
        const deadline = AbortSignal.timeout(PROBE_TIMEOUT_MS);
        // One signal reaches the transport: the deadline forwards its abort
        // into the controller, and an operator cancel aborts the same
        // controller directly — whichever fires first ends the request.
        deadline.addEventListener("abort", () => {
            controller.abort(deadline.reason);
        });
        isProbeCancelled = false;
        inFlightProbe = controller;

        try {
            // The probe is the setup button's busy state: without a deadline
            // a dead address would hold it for the browser's full default
            // fetch timeout. The deadline surfaces as the usual transport
            // failure, which the setup panel diagnoses like any other
            // refusal; an operator cancel surfaces as
            // `ConnectionProbeCancelled`, which the setup screen never
            // renders as a refusal.
            await createManagementApi(connection).listProviders(controller.signal);
        } catch (error) {
            if (isProbeCancelled) {
                throw new ConnectionProbeCancelled();
            }

            throw error;
        } finally {
            inFlightProbe = null;
        }
    },
});
