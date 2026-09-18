import { connectionService } from "/src/connection/app-service";
import { type ActionName, type Diagnosis, diagnoseFailure } from "/src/management-api/diagnosis";

/**
 * Explains a failed management call made from an established connection: the
 * address comes from the live connection and the origin from this page, so a
 * screen only names the action it attempted. No reachability probe runs here —
 * a mid-session failure cannot be told apart from a dropped network, and the
 * diagnosis says so rather than guessing.
 */
export function describeCallFailure(error: unknown, action?: ActionName): Diagnosis {
    return diagnoseFailure(error, {
        baseUrl: connectionService.currentConnection()?.baseUrl ?? "",
        pageOrigin: window.location.origin,
        action,
    });
}
