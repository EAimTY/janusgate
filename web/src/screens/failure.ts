import { connectionService } from "/src/connection/app-service";
import { type ActionName, type Diagnosis, diagnoseFailure } from "/src/management-api/diagnosis";
import type { ProviderType } from "/src/management-api/generated/v0";

/** The management call whose failure is being explained. */
export type FailureCall = {
    /** The action the call attempted, which several diagnoses specialize on. */
    readonly action?: ActionName | undefined;
    /** The provider type the call addressed, which a discovery failure's fix specializes on. */
    readonly providerType?: ProviderType | undefined;
};

/**
 * Explains a failed management call made from an established connection: the
 * address comes from the live connection and the origin from this page, so a
 * screen only names the call it attempted. No reachability probe runs here —
 * a mid-session failure cannot be told apart from a dropped network, and the
 * diagnosis says so rather than guessing.
 */
export function describeCallFailure(error: unknown, call?: FailureCall): Diagnosis {
    return diagnoseFailure(error, {
        baseUrl: connectionService.currentConnection()?.baseUrl ?? "",
        pageOrigin: window.location.origin,
        action: call?.action,
        providerType: call?.providerType,
    });
}
