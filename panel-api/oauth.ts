import {
    ANTIGRAVITY_API_BASE_URL,
    ANTIGRAVITY_API_CLIENT,
    ANTIGRAVITY_AUTHORIZE_URL,
    ANTIGRAVITY_CLIENT_ID,
    ANTIGRAVITY_CLIENT_METADATA,
    ANTIGRAVITY_CLIENT_SECRET,
    ANTIGRAVITY_HEADER_CLIENT_METADATA,
    ANTIGRAVITY_LOAD_CODE_ASSIST_URL,
    ANTIGRAVITY_OAUTH_SCOPE,
    ANTIGRAVITY_ONBOARD_USER_URL,
    ANTIGRAVITY_REDIRECT_URI,
    ANTIGRAVITY_TOKEN_URL,
    ANTIGRAVITY_USER_AGENT,
} from "/api/adapters/antigravity";
import {
    OPENAI_AUTHORIZE_URL,
    OPENAI_OAUTH_CLIENT_ID,
    OPENAI_OAUTH_SCOPE,
    OPENAI_REDIRECT_URI,
    OPENAI_TOKEN_URL,
} from "/api/adapters/openai";
import { REFRESH_LEASE_SECONDS } from "/api/config";
import type { CredentialsCipher } from "/common/credentials";
import type { StoredProvider } from "/common/database";
import type { OAuthFlowType, OpenAiOAuthCredentials, ProviderCredentials } from "/common/domain";
import { isJsonObject, unixSeconds } from "/common/validation";
import { MILLISECONDS_PER_SECOND, OAUTH_UPSTREAM_TIMEOUT_SECONDS } from "/config";
import {
    ANTIGRAVITY_ONBOARDING_MAX_POLLS,
    ANTIGRAVITY_ONBOARDING_POLL_INTERVAL_MS,
    OAUTH_RANDOM_VALUE_BYTES,
    OAUTH_STATE_TTL_SECONDS,
} from "./config";
import { isDatabaseConstraintError, PanelApiError } from "./errors";
import type { CompleteOAuthInput, StartOAuthInput } from "./input";
import {
    findProviderById,
    generateAvailableProviderName,
    requireProviderByName,
} from "./provider-repository";

const PKCE_DIGEST_ALGORITHM = "SHA-256";

interface OAuthStateRow {
    id: number;
    type: string;
    state: string;
    verifier: string;
    provider_id: number | null;
    expires_at: number;
}

interface OAuthTimeOptions {
    readonly now?: number;
}

interface OAuthCompletionOptions extends OAuthTimeOptions {
    readonly fetchUpstream?: typeof fetch;
    readonly signal?: AbortSignal;
    readonly sleep?: (milliseconds: number) => Promise<void>;
}

interface AuthorizationToken {
    readonly accessToken: string;
    readonly refreshToken: string;
    readonly expiresAt: number;
    readonly idToken?: string;
}

function providerTypeForFlow(type: OAuthFlowType): "antigravity_oauth" | "openai_oauth" {
    return type === "openai" ? "openai_oauth" : "antigravity_oauth";
}

function base64UrlEncode(bytes: Uint8Array): string {
    let binary = "";
    for (const byte of bytes) binary += String.fromCharCode(byte);
    return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}

function randomBase64Url(): string {
    return base64UrlEncode(crypto.getRandomValues(new Uint8Array(OAUTH_RANDOM_VALUE_BYTES)));
}

function oauthUpstreamSignal(signal?: AbortSignal): AbortSignal {
    const timeoutSignal = AbortSignal.timeout(
        OAUTH_UPSTREAM_TIMEOUT_SECONDS * MILLISECONDS_PER_SECOND,
    );
    return signal === undefined
        ? timeoutSignal
        : AbortSignal.any([
              signal,
              timeoutSignal,
          ]);
}

function sleepWithSignal(milliseconds: number, signal?: AbortSignal): Promise<void> {
    if (signal?.aborted === true) return Promise.reject(signal.reason);
    return new Promise((resolve, reject) => {
        const timeout = setTimeout(() => {
            signal?.removeEventListener("abort", onAbort);
            resolve();
        }, milliseconds);
        const onAbort = (): void => {
            clearTimeout(timeout);
            reject(signal?.reason);
        };
        signal?.addEventListener("abort", onAbort, {
            once: true,
        });
    });
}

async function createPkce(): Promise<{
    verifier: string;
    challenge: string;
}> {
    const verifier = randomBase64Url();
    const digest = new Uint8Array(
        await crypto.subtle.digest(PKCE_DIGEST_ALGORITHM, new TextEncoder().encode(verifier)),
    );
    return {
        challenge: base64UrlEncode(digest),
        verifier,
    };
}

function buildAuthorizeUrl(type: OAuthFlowType, state: string, challenge: string): string {
    const isOpenAi = type === "openai";
    const url = new URL(isOpenAi ? OPENAI_AUTHORIZE_URL : ANTIGRAVITY_AUTHORIZE_URL);
    url.searchParams.set("client_id", isOpenAi ? OPENAI_OAUTH_CLIENT_ID : ANTIGRAVITY_CLIENT_ID);
    url.searchParams.set("response_type", "code");
    url.searchParams.set("redirect_uri", isOpenAi ? OPENAI_REDIRECT_URI : ANTIGRAVITY_REDIRECT_URI);
    url.searchParams.set("scope", isOpenAi ? OPENAI_OAUTH_SCOPE : ANTIGRAVITY_OAUTH_SCOPE);
    url.searchParams.set("code_challenge", challenge);
    url.searchParams.set("code_challenge_method", "S256");
    url.searchParams.set("state", state);
    if (!isOpenAi) {
        url.searchParams.set("access_type", "offline");
        url.searchParams.set("prompt", "consent");
    }
    return url.toString();
}

export async function startProviderOAuth(
    database: D1Database,
    input: StartOAuthInput,
    options: OAuthTimeOptions = {},
): Promise<string> {
    let providerId: number | null = null;
    if (input.providerName !== undefined) {
        const provider = await requireProviderByName(database, input.providerName);
        if (provider.type !== providerTypeForFlow(input.type)) {
            throw new PanelApiError(400, "Provider type does not match the OAuth flow");
        }
        providerId = provider.id;
    }

    const state = randomBase64Url();
    const pkce = await createPkce();
    const now = options.now ?? unixSeconds();
    await database
        .prepare(
            `INSERT INTO oauth_states (
                type, state, verifier, provider_id, expires_at
            ) VALUES (?, ?, ?, ?, ?)`,
        )
        .bind(input.type, state, pkce.verifier, providerId, now + OAUTH_STATE_TTL_SECONDS)
        .run();
    return buildAuthorizeUrl(input.type, state, pkce.challenge);
}

function parseRedirectUrl(value: string): URL {
    try {
        return new URL(value);
    } catch {
        throw new PanelApiError(400, "redirectUrl must be a valid URL");
    }
}

function parseStoredFlowType(value: string): OAuthFlowType {
    if (value === "openai" || value === "antigravity") return value;
    throw new Error("Invalid stored OAuth flow type");
}

function assertMatchingRedirect(url: URL, type: OAuthFlowType): void {
    const expected = new URL(type === "openai" ? OPENAI_REDIRECT_URI : ANTIGRAVITY_REDIRECT_URI);
    if (
        url.protocol !== expected.protocol ||
        url.hostname !== expected.hostname ||
        url.port !== expected.port ||
        url.pathname !== expected.pathname ||
        url.username !== "" ||
        url.password !== ""
    ) {
        throw new PanelApiError(400, "redirectUrl does not match this OAuth flow");
    }
}

async function deleteState(database: D1Database, stateId: number): Promise<void> {
    await database.prepare("DELETE FROM oauth_states WHERE id = ?").bind(stateId).run();
}

async function readJsonObjectResponse(
    response: Response,
    context: string,
): Promise<Record<string, unknown>> {
    if (!response.ok) {
        throw new PanelApiError(502, `${context} failed with HTTP ${response.status}`);
    }
    let value: unknown;
    try {
        value = await response.json();
    } catch {
        throw new PanelApiError(502, `${context} returned invalid JSON`);
    }
    if (!isJsonObject(value)) {
        throw new PanelApiError(502, `${context} returned an invalid response`);
    }
    return value;
}

function requireResponseString(
    object: Record<string, unknown>,
    fieldName: string,
    context: string,
): string {
    const value = object[fieldName];
    if (typeof value !== "string") {
        throw new PanelApiError(502, `${context} returned an invalid response`);
    }
    return value;
}

function requireResponseNumber(
    object: Record<string, unknown>,
    fieldName: string,
    context: string,
): number {
    const value = object[fieldName];
    if (typeof value !== "number") {
        throw new PanelApiError(502, `${context} returned an invalid response`);
    }
    return value;
}

async function exchangeAuthorizationCode(
    type: OAuthFlowType,
    code: string,
    verifier: string,
    now: number,
    fetchUpstream: typeof fetch,
    signal?: AbortSignal,
): Promise<AuthorizationToken> {
    const isOpenAi = type === "openai";
    const parameters = new URLSearchParams({
        client_id: isOpenAi ? OPENAI_OAUTH_CLIENT_ID : ANTIGRAVITY_CLIENT_ID,
        code,
        code_verifier: verifier,
        grant_type: "authorization_code",
        redirect_uri: isOpenAi ? OPENAI_REDIRECT_URI : ANTIGRAVITY_REDIRECT_URI,
    });
    if (!isOpenAi) parameters.set("client_secret", ANTIGRAVITY_CLIENT_SECRET);

    let response: Response;
    try {
        response = await fetchUpstream(isOpenAi ? OPENAI_TOKEN_URL : ANTIGRAVITY_TOKEN_URL, {
            body: parameters,
            headers: {
                "Content-Type": "application/x-www-form-urlencoded",
            },
            method: "POST",
            signal: oauthUpstreamSignal(signal),
        });
    } catch {
        throw new PanelApiError(502, "OAuth token exchange failed");
    }
    const value = await readJsonObjectResponse(response, "OAuth token exchange");
    const token: {
        accessToken: string;
        refreshToken: string;
        expiresAt: number;
        idToken?: string;
    } = {
        accessToken: requireResponseString(value, "access_token", "OAuth token exchange"),
        expiresAt: now + requireResponseNumber(value, "expires_in", "OAuth token exchange"),
        refreshToken: requireResponseString(value, "refresh_token", "OAuth token exchange"),
    };
    if (isOpenAi) {
        token.idToken = requireResponseString(value, "id_token", "OAuth token exchange");
    }
    return token;
}

function decodeJwtPayload(token: string): Record<string, unknown> {
    const parts = token.split(".");
    const payload = parts[1];
    if (parts.length !== 3 || payload === undefined || !/^[A-Za-z0-9_-]+$/.test(payload)) {
        throw new PanelApiError(502, "OpenAI token exchange returned an invalid id_token");
    }
    const standardBase64 = payload.replaceAll("-", "+").replaceAll("_", "/");
    const padded = standardBase64.padEnd(Math.ceil(standardBase64.length / 4) * 4, "=");
    try {
        const binary = atob(padded);
        const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
        const decoded = new TextDecoder("utf-8", {
            fatal: true,
            ignoreBOM: false,
        }).decode(bytes);
        const value: unknown = JSON.parse(decoded);
        if (!isJsonObject(value)) throw new Error("JWT payload is not an object");
        return value;
    } catch {
        throw new PanelApiError(502, "OpenAI token exchange returned an invalid id_token");
    }
}

function openAiCredentials(token: AuthorizationToken): OpenAiOAuthCredentials {
    if (token.idToken === undefined) {
        throw new Error("OpenAI token is missing its id_token");
    }
    const claims = decodeJwtPayload(token.idToken);
    return {
        accessToken: token.accessToken,
        accountId: requireResponseString(claims, "chatgpt_account_id", "OpenAI id_token"),
        refreshToken: token.refreshToken,
    };
}

function antigravityHeaders(accessToken: string): Record<string, string> {
    return {
        Accept: "*/*",
        Authorization: `Bearer ${accessToken}`,
        "Client-Metadata": ANTIGRAVITY_HEADER_CLIENT_METADATA,
        "Content-Type": "application/json",
        "User-Agent": ANTIGRAVITY_USER_AGENT,
        "X-Goog-Api-Client": ANTIGRAVITY_API_CLIENT,
    };
}

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
                headers: antigravityHeaders(accessToken),
                method: "GET",
                signal: oauthUpstreamSignal(signal),
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

async function resolveAntigravityProject(
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
            headers: antigravityHeaders(accessToken),
            method: "POST",
            signal: oauthUpstreamSignal(signal),
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
            headers: antigravityHeaders(accessToken),
            method: "POST",
            signal: oauthUpstreamSignal(signal),
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

async function completionCredentials(
    flowType: OAuthFlowType,
    token: AuthorizationToken,
    fetchUpstream: typeof fetch,
    sleep: (milliseconds: number) => Promise<void>,
    signal?: AbortSignal,
    explicitProjectId?: string,
): Promise<ProviderCredentials> {
    if (flowType === "openai") return openAiCredentials(token);

    // Reauthentication may select a different Google account. Always resolve the project from
    // the newly issued access token; reusing the previous account's project causes 400/403 calls.
    return {
        accessToken: token.accessToken,
        projectId: await resolveAntigravityProject(
            token.accessToken,
            fetchUpstream,
            sleep,
            signal,
            explicitProjectId,
        ),
        refreshToken: token.refreshToken,
    };
}

async function acquireReauthenticationLease(
    database: D1Database,
    providerId: number,
    now: number,
): Promise<boolean> {
    const result = await database
        .prepare(
            `UPDATE providers SET refresh_started_at = ?
            WHERE id = ?
                AND (refresh_started_at IS NULL OR refresh_started_at < ?)`,
        )
        .bind(now, providerId, now - REFRESH_LEASE_SECONDS)
        .run();
    return result.meta.changes === 1;
}

async function clearReauthenticationLease(
    database: D1Database,
    providerId: number,
    leaseTimestamp: number,
): Promise<void> {
    await database
        .prepare(
            `UPDATE providers SET refresh_started_at = NULL
            WHERE id = ? AND refresh_started_at = ?`,
        )
        .bind(providerId, leaseTimestamp)
        .run();
}

async function createOAuthProviderAtomically(
    database: D1Database,
    state: OAuthStateRow,
    flowType: OAuthFlowType,
    input: CompleteOAuthInput,
    encryptedCredentials: Uint8Array,
    expiresAt: number,
): Promise<void> {
    const providerType = providerTypeForFlow(flowType);
    const name = input.name ?? (await generateAvailableProviderName(database, providerType));
    const modelPrefix = input.modelPrefix ?? name;
    const results = await database.batch([
        database
            .prepare(
                `INSERT INTO providers (
                    name, model_prefix, type, protocol, base_url,
                    encrypted_credentials, expires_at, refresh_started_at, enabled
                )
                SELECT ?, ?, ?, NULL, NULL, ?, ?, NULL, 1
                WHERE EXISTS (
                    SELECT 1 FROM oauth_states WHERE id = ? AND state = ?
                )`,
            )
            .bind(
                name,
                modelPrefix,
                providerType,
                encryptedCredentials,
                expiresAt,
                state.id,
                state.state,
            ),
        database
            .prepare("DELETE FROM oauth_states WHERE id = ? AND state = ?")
            .bind(state.id, state.state),
    ]);
    if (results[1]?.meta.changes !== 1 || results[0]?.meta.changes !== 1) {
        throw new PanelApiError(400, "OAuth state was already used");
    }
}

async function reauthenticateProviderAtomically(
    database: D1Database,
    state: OAuthStateRow,
    provider: StoredProvider,
    encryptedCredentials: Uint8Array,
    expiresAt: number,
    leaseTimestamp: number,
): Promise<void> {
    const results = await database.batch([
        database
            .prepare(
                `UPDATE providers SET
                    encrypted_credentials = ?, expires_at = ?, refresh_started_at = NULL
                WHERE id = ? AND refresh_started_at = ?
                    AND EXISTS (
                        SELECT 1 FROM oauth_states WHERE id = ? AND state = ?
                    )`,
            )
            .bind(
                encryptedCredentials,
                expiresAt,
                provider.id,
                leaseTimestamp,
                state.id,
                state.state,
            ),
        database
            .prepare("DELETE FROM oauth_states WHERE id = ? AND state = ?")
            .bind(state.id, state.state),
    ]);
    if (results[1]?.meta.changes !== 1) {
        throw new PanelApiError(400, "OAuth state was already used");
    }
    if (results[0]?.meta.changes !== 1) {
        throw new PanelApiError(409, "Provider credentials changed concurrently; restart OAuth");
    }
}

export async function completeProviderOAuth(
    database: D1Database,
    cipher: CredentialsCipher,
    input: CompleteOAuthInput,
    options: OAuthCompletionOptions = {},
): Promise<void> {
    const now = options.now ?? unixSeconds();
    const redirectUrl = parseRedirectUrl(input.redirectUrl);
    const stateValue = redirectUrl.searchParams.get("state");
    if (stateValue === null || stateValue.length === 0) {
        throw new PanelApiError(400, "OAuth redirect is missing state");
    }

    // One completion request performs the entire opportunistic expiry sweep.
    await database.prepare("DELETE FROM oauth_states WHERE expires_at <= ?").bind(now).run();
    const state = await database
        .prepare(
            `SELECT id, type, state, verifier, provider_id, expires_at
            FROM oauth_states WHERE state = ?`,
        )
        .bind(stateValue)
        .first<OAuthStateRow>();
    if (state === null || state.expires_at <= now) {
        throw new PanelApiError(400, "OAuth state is invalid or expired");
    }

    const flowType = parseStoredFlowType(state.type);
    let leasedProvider: StoredProvider | null = null;
    let hasLease = false;
    try {
        assertMatchingRedirect(redirectUrl, flowType);
        if (redirectUrl.searchParams.has("error")) {
            throw new PanelApiError(400, "OAuth authorization was denied");
        }
        const code = redirectUrl.searchParams.get("code");
        if (code === null || code.length === 0) {
            throw new PanelApiError(400, "OAuth redirect is missing code");
        }

        if (state.provider_id !== null) {
            leasedProvider = await findProviderById(database, state.provider_id);
            if (leasedProvider === null) throw new PanelApiError(404, "Provider not found");
            if (leasedProvider.type !== providerTypeForFlow(flowType)) {
                throw new PanelApiError(400, "Provider type does not match the OAuth flow");
            }
        }

        if (flowType === "openai" && input.projectId !== undefined) {
            throw new PanelApiError(400, "projectId applies only to Antigravity OAuth");
        }
        const fetchUpstream = options.fetchUpstream ?? fetch;
        const token = await exchangeAuthorizationCode(
            flowType,
            code,
            state.verifier,
            now,
            fetchUpstream,
            options.signal,
        );
        const credentials = await completionCredentials(
            flowType,
            token,
            fetchUpstream,
            options.sleep ?? ((milliseconds) => sleepWithSignal(milliseconds, options.signal)),
            options.signal,
            input.projectId,
        );
        const encryptedCredentials = await cipher.encrypt(credentials);

        if (leasedProvider === null) {
            await createOAuthProviderAtomically(
                database,
                state,
                flowType,
                input,
                encryptedCredentials,
                token.expiresAt,
            );
            return;
        }

        hasLease = await acquireReauthenticationLease(database, leasedProvider.id, now);
        if (!hasLease) {
            throw new PanelApiError(409, "Provider credential mutation is already in progress");
        }
        await reauthenticateProviderAtomically(
            database,
            state,
            leasedProvider,
            encryptedCredentials,
            token.expiresAt,
            now,
        );
    } catch (error: unknown) {
        if (hasLease && leasedProvider !== null) {
            await clearReauthenticationLease(database, leasedProvider.id, now);
        }
        await deleteState(database, state.id);
        if (isDatabaseConstraintError(error, "providers.name")) {
            throw new PanelApiError(409, "Provider name already exists");
        }
        throw error;
    }
}
