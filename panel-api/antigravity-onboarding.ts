import { boundedUpstreamSignal } from "/common/upstream-signal";
import { isJsonObject } from "/common/validation";
import {
    ANTIGRAVITY_API_BASE_URL,
    ANTIGRAVITY_CLIENT_METADATA,
    ANTIGRAVITY_LOAD_CODE_ASSIST_URL,
    ANTIGRAVITY_ONBOARD_USER_URL,
    antigravityRequestHeaders,
} from "/providers/antigravity";
import {
    ANTIGRAVITY_ONBOARDING_MAX_POLLS,
    ANTIGRAVITY_ONBOARDING_POLL_INTERVAL_MS,
} from "./config";
import { PanelApiError } from "./errors";
import { readJsonObjectResponse } from "./upstream-json";

/**
 * §8.4 project resolution/onboarding: provision the selected Google account for Code Assist
 * when needed and resolve the project ID stored with the credentials. Reauthentication always
 * resolves from the newly issued access token because the admin may select a different
 * Google account; reusing the previous account's project causes 400/403 upstream calls.
 */

function antigravityMetadata(): Record<string, unknown> {
    let value: unknown;
    try {
        value = JSON.parse(ANTIGRAVITY_CLIENT_METADATA);
    } catch {
        throw new Error("Invalid pinned Antigravity client metadata");
    }
    if (!isJsonObject(value)) throw new Error("Invalid pinned Antigravity client metadata");
    return value;
}

function extractProjectId(value: Record<string, unknown>): string | undefined {
    for (const key of [
        "antigravityProjectId",
        "projectId",
        "backendProjectId",
        "userDefinedCloudaicompanionProject",
        "cloudaicompanionProject",
        "project",
    ]) {
        const project = value[key];
        if (typeof project === "string" && project.length > 0) return project;
        if (!isJsonObject(project)) continue;
        for (const nestedKey of [
            "id",
            "projectId",
            "project",
        ]) {
            const nested = project[nestedKey];
            if (typeof nested === "string" && nested.length > 0) return nested;
        }
    }
    return undefined;
}

function selectOnboardingTier(loadResponse: Record<string, unknown>): {
    id: string;
    isProjectRequired: boolean;
} {
    const ineligibleTiers = loadResponse["ineligibleTiers"];
    if (Array.isArray(ineligibleTiers) && ineligibleTiers.length > 0) {
        for (const tier of ineligibleTiers) {
            if (
                !isJsonObject(tier) ||
                tier["reasonCode"] !== "VALIDATION_REQUIRED" ||
                typeof tier["validationUrl"] !== "string"
            )
                continue;
            let validationUrl: URL;
            try {
                validationUrl = new URL(tier["validationUrl"]);
            } catch {
                continue;
            }
            if (validationUrl.protocol !== "https:") continue;
            throw new PanelApiError(
                400,
                `Google account validation is required before connecting: ${validationUrl.toString()}`,
            );
        }
    }
    const tiers = loadResponse["allowedTiers"];
    if (Array.isArray(tiers)) {
        for (const tier of tiers) {
            if (
                isJsonObject(tier) &&
                tier["isDefault"] === true &&
                typeof tier["id"] === "string"
            ) {
                return {
                    id: tier["id"],
                    isProjectRequired: tier["id"] !== "free-tier",
                };
            }
        }
    }
    return {
        id: "legacy-tier",
        isProjectRequired: true,
    };
}

function antigravityOperationUrl(name: string): string {
    const segments = name.split("/");
    if (
        segments.length < 2 ||
        segments[0] !== "operations" ||
        segments.some((segment) => !/^[A-Za-z0-9._~-]+$/.test(segment))
    ) {
        throw new PanelApiError(502, "Antigravity onboarding returned an invalid operation");
    }
    return `${ANTIGRAVITY_API_BASE_URL}/${segments.map(encodeURIComponent).join("/")}`;
}

async function pollAntigravityOnboarding(
    initialOperation: Record<string, unknown>,
    accessToken: string,
    fetchUpstream: typeof fetch,
    sleep: (milliseconds: number) => Promise<void>,
    signal?: AbortSignal,
): Promise<Record<string, unknown>> {
    let operation = initialOperation;
    for (let pollCount = 0; operation["done"] !== true; pollCount += 1) {
        if (pollCount >= ANTIGRAVITY_ONBOARDING_MAX_POLLS) {
            throw new PanelApiError(502, "Antigravity onboarding timed out");
        }
        const name = operation["name"];
        if (typeof name !== "string") {
            throw new PanelApiError(502, "Antigravity onboarding returned an invalid operation");
        }
        await sleep(ANTIGRAVITY_ONBOARDING_POLL_INTERVAL_MS);
        let response: Response;
        try {
            response = await fetchUpstream(antigravityOperationUrl(name), {
                headers: antigravityRequestHeaders(accessToken, "*/*"),
                method: "GET",
                signal: boundedUpstreamSignal(signal),
            });
        } catch {
            throw new PanelApiError(502, "Antigravity onboarding operation failed");
        }
        operation = await readJsonObjectResponse(response, "Antigravity onboarding operation");
    }
    if (operation["error"] !== undefined) {
        throw new PanelApiError(502, "Antigravity onboarding operation failed");
    }
    return operation;
}

async function isVpcServiceControlsRejection(response: Response): Promise<boolean> {
    if (response.status !== 403) return false;
    let value: unknown;
    try {
        value = await response.clone().json();
    } catch {
        return false;
    }
    if (!isJsonObject(value) || !isJsonObject(value["error"])) return false;
    const details = value["error"]["details"];
    return (
        Array.isArray(details) &&
        details.some(
            (detail) => isJsonObject(detail) && detail["reason"] === "SECURITY_POLICY_VIOLATED",
        )
    );
}

export async function resolveAntigravityProject(
    accessToken: string,
    fetchUpstream: typeof fetch,
    sleep: (milliseconds: number) => Promise<void>,
    signal?: AbortSignal,
    explicitProjectId?: string,
): Promise<string> {
    const metadata = antigravityMetadata();
    if (explicitProjectId !== undefined) metadata["duetProject"] = explicitProjectId;
    let loadResponse: Response;
    try {
        loadResponse = await fetchUpstream(ANTIGRAVITY_LOAD_CODE_ASSIST_URL, {
            body: JSON.stringify({
                ...(explicitProjectId === undefined
                    ? {}
                    : {
                          cloudaicompanionProject: explicitProjectId,
                      }),
                metadata,
            }),
            headers: antigravityRequestHeaders(accessToken, "*/*"),
            method: "POST",
            signal: boundedUpstreamSignal(signal),
        });
    } catch {
        throw new PanelApiError(502, "Antigravity project resolution failed");
    }
    if (explicitProjectId !== undefined && (await isVpcServiceControlsRejection(loadResponse))) {
        return explicitProjectId;
    }
    const load = await readJsonObjectResponse(loadResponse, "Antigravity project resolution");
    const existingProjectId = extractProjectId(load);
    if (existingProjectId !== undefined) return existingProjectId;
    if (isJsonObject(load["currentTier"])) {
        if (explicitProjectId !== undefined) return explicitProjectId;
        throw new PanelApiError(
            400,
            "This Google account requires an explicit Google Cloud project ID",
        );
    }

    const tier = selectOnboardingTier(load);
    if (tier.isProjectRequired && explicitProjectId === undefined) {
        throw new PanelApiError(
            400,
            "This Google account requires an explicit Google Cloud project ID",
        );
    }
    const onboardingProjectId = tier.isProjectRequired ? explicitProjectId : undefined;
    const onboardingMetadata = antigravityMetadata();
    if (onboardingProjectId !== undefined) onboardingMetadata["duetProject"] = onboardingProjectId;
    let onboardResponse: Response;
    try {
        onboardResponse = await fetchUpstream(ANTIGRAVITY_ONBOARD_USER_URL, {
            body: JSON.stringify({
                ...(onboardingProjectId === undefined
                    ? {}
                    : {
                          cloudaicompanionProject: onboardingProjectId,
                      }),
                metadata: onboardingMetadata,
                tierId: tier.id,
            }),
            headers: antigravityRequestHeaders(accessToken, "*/*"),
            method: "POST",
            signal: boundedUpstreamSignal(signal),
        });
    } catch {
        throw new PanelApiError(502, "Antigravity onboarding failed");
    }
    const initialOperation = await readJsonObjectResponse(
        onboardResponse,
        "Antigravity onboarding",
    );
    const onboard = await pollAntigravityOnboarding(
        initialOperation,
        accessToken,
        fetchUpstream,
        sleep,
        signal,
    );
    if (!isJsonObject(onboard["response"])) {
        throw new PanelApiError(502, "Antigravity onboarding returned an invalid response");
    }
    const projectId = extractProjectId(onboard["response"]);
    if (projectId !== undefined) return projectId;
    if (onboardingProjectId !== undefined) return onboardingProjectId;
    throw new PanelApiError(502, "Antigravity onboarding returned an invalid response");
}
