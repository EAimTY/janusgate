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

/**
 * The application's single connection service, wired to the browser's storage
 * and to the connection probe — `providers:list`, the first call of README
 * §3.1.
 */
export const connectionService = createConnectionService({
    storage: browserStorage(),
    probe: async (connection: Connection) => {
        await createManagementApi(connection).listProviders();
    },
});
