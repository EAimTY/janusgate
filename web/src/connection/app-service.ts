import { appConfig } from "/src/config";
import type { Connection } from "/src/connection/model";
import { createConnectionService } from "/src/connection/service";
import { createManagementApi } from "/src/management-api/client";
import { deadlineController } from "./deadline";

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

/**
 * Cancels the connect probe in flight, if one is, through the service's own
 * cancel channel: the attempt ends and the machine records a cancellation —
 * control flow, never a refusal. A probe that already answered is untouched.
 */
export function cancelConnectionProbe(): void {
    connectionService.cancelConnect();
}

/**
 * The application's single connection service, wired to the browser's storage
 * and to the connection probe — `providers:list`, the first call of README
 * §3.1. The service owns the attempt's cancel channel: `cancelConnect` aborts
 * the signal the probe in flight carries, and an aborted attempt lands as the
 * machine's cancellation, never as a refusal.
 */
export const connectionService = createConnectionService({
    storage: browserStorage(),
    probe: async (connection: Connection, signal: AbortSignal) => {
        const controller = deadlineController(appConfig.connectionProbeTimeoutMs);
        // The service's cancel forwards into the probe's own deadline
        // controller, so the transport call ends on whichever fires first.
        // The deadline's expiry rejects like any transport failure and the
        // setup panel diagnoses it as a refusal; the operator's cancel is
        // visible to the service alone, as its own signal's abort — the
        // distinction that lands an attempt as a cancellation instead.
        signal.addEventListener("abort", () => controller.abort(signal.reason), {
            once: true,
        });

        await createManagementApi(connection).listProviders(controller.signal);
    },
});
