import { verifyBearerAuthorization } from "/common/auth";
import { CredentialsCipher } from "/common/credentials";
import {
    GatewayError,
    gatewayErrorBody,
    internalGatewayError,
    invalidApiKeyError,
    modelNotFoundError,
    upstreamFailureError,
    upstreamTimeoutError,
} from "/common/errors";
import { readBoundedJsonObject } from "/common/http";
import { isJsonObject } from "/common/validation";
import {
    FORCED_REFRESH_RETRY_COUNT,
    MAX_REQUEST_BYTES,
    UPSTREAM_IDLE_TIMEOUT_SECONDS,
} from "./config";
import { type ResolvedModelRoute, resolveEnabledModel } from "./model-routing";
import { ensureFreshProviderAccess, type OAuthCredentials } from "./provider-access";
import { type ClientProtocol, validateGatewayRequest } from "./request-validation";
import { renderTranslatedNonStreamingResponse } from "./translated-nonstream";
import { translateStreamingResponse } from "./translated-stream";
import { TranslationError } from "./translation/errors";
import { readIdleTimedUpstreamBody } from "./upstream-body";
import { type PreparedUpstreamRequest, prepareUpstreamRequest } from "./upstream-request";
import { copySafeUpstreamHeaders, relayNativeStreamingResponse } from "./upstream-response";

function gatewayErrorResponse(error: GatewayError, headers?: Headers): Response {
    return headers === undefined
        ? Response.json(gatewayErrorBody(error), {
              status: error.status,
          })
        : Response.json(gatewayErrorBody(error), {
              headers,
              status: error.status,
          });
}

function mapTranslationError(error: TranslationError): GatewayError {
    return error.code === "upstream_failure"
        ? upstreamFailureError()
        : new GatewayError(400, "invalid_request_error", error.message, error.code);
}

function isUsageIncluded(body: Record<string, unknown>): boolean {
    const streamOptions = body["stream_options"];
    return isJsonObject(streamOptions) && streamOptions["include_usage"] === true;
}

async function fetchWithHeaderTimeout(
    request: PreparedUpstreamRequest,
    abortController: AbortController,
    fetchUpstream: typeof fetch,
): Promise<Response> {
    const timeout = setTimeout(
        () => abortController.abort("upstream idle timeout"),
        UPSTREAM_IDLE_TIMEOUT_SECONDS * 1_000,
    );
    try {
        return await fetchUpstream(request.url, {
            body: request.body,
            headers: request.headers,
            method: "POST",
            signal: abortController.signal,
        });
    } catch (error) {
        if (abortController.signal.reason === "upstream idle timeout") {
            throw upstreamTimeoutError();
        }
        throw error;
    } finally {
        clearTimeout(timeout);
    }
}

async function executeUpstream(
    clientRequest: Request,
    validatedRequest: ReturnType<typeof validateGatewayRequest>,
    protocol: ClientProtocol,
    route: ResolvedModelRoute,
    database: D1Database,
    cipher: CredentialsCipher,
    abortController: AbortController,
    fetchUpstream: typeof fetch,
): Promise<{
    prepared: PreparedUpstreamRequest;
    response: Response;
}> {
    let oauthCredentials: OAuthCredentials | undefined;
    let currentRoute = route;
    if (route.provider.type !== "openai_compatible") {
        const access = await ensureFreshProviderAccess(database, route.provider, cipher, {
            fetchUpstream,
        });
        oauthCredentials = access.credentials;
        currentRoute = {
            ...route,
            provider: access.provider,
        };
    }
    let prepared = await prepareUpstreamRequest(
        clientRequest,
        validatedRequest,
        protocol,
        currentRoute,
        cipher,
        oauthCredentials,
    );
    let response = await fetchWithHeaderTimeout(prepared, abortController, fetchUpstream);
    for (
        let retryCount = 0;
        response.status === 401 &&
        currentRoute.provider.type !== "openai_compatible" &&
        retryCount < FORCED_REFRESH_RETRY_COUNT;
        retryCount += 1
    ) {
        await response.body?.cancel();
        const refreshed = await ensureFreshProviderAccess(database, currentRoute.provider, cipher, {
            fetchUpstream,
            isForced: true,
        });
        currentRoute = {
            ...route,
            provider: refreshed.provider,
        };
        prepared = await prepareUpstreamRequest(
            clientRequest,
            validatedRequest,
            protocol,
            currentRoute,
            cipher,
            refreshed.credentials,
        );
        response = await fetchWithHeaderTimeout(prepared, abortController, fetchUpstream);
    }
    if (response.status === 401 && currentRoute.provider.type !== "openai_compatible") {
        await response.body?.cancel();
        throw new GatewayError(
            502,
            "upstream_error",
            "The upstream rejected authentication",
            "upstream_auth_rejected",
        );
    }
    return {
        prepared,
        response,
    };
}

async function relayNativeNonStreaming(
    response: Response,
    abortController: AbortController,
): Promise<Response> {
    const bytes = await readIdleTimedUpstreamBody(response, abortController);
    return new Response(bytes, {
        headers: copySafeUpstreamHeaders(response.headers),
        status: response.status,
    });
}

async function translatedUpstreamError(
    response: Response,
    route: ResolvedModelRoute,
    abortController: AbortController,
): Promise<Response> {
    const status = response.status === 429 ? 429 : 502;
    const headers = new Headers();
    const retryAfter = response.headers.get("Retry-After");
    if (retryAfter !== null) headers.set("Retry-After", retryAfter);
    if (route.provider.type === "openai_compatible") {
        const contentType = response.headers.get("Content-Type");
        if (contentType !== null) headers.set("Content-Type", contentType);
        const bytes = await readIdleTimedUpstreamBody(response, abortController);
        return new Response(bytes, {
            headers,
            status,
        });
    }
    await response.body?.cancel();
    return gatewayErrorResponse(
        new GatewayError(
            status,
            status === 429 ? "rate_limit_error" : "upstream_error",
            status === 429 ? "The upstream rate limit was reached" : "The upstream request failed",
            status === 429 ? undefined : "upstream_failure",
        ),
        headers,
    );
}

async function handleGatewayChat(
    request: Request,
    env: Env,
    protocol: ClientProtocol,
    fetchUpstream: typeof fetch,
): Promise<Response> {
    if (
        !(await verifyBearerAuthorization(request.headers.get("Authorization"), env.API_KEY_HASH))
    ) {
        throw invalidApiKeyError();
    }
    const body = await readBoundedJsonObject(request, MAX_REQUEST_BYTES);
    const initial = validateGatewayRequest(body, protocol);
    const route = await resolveEnabledModel(env.DATABASE, initial.requestedModel);
    if (route === null) throw modelNotFoundError();
    const validated = validateGatewayRequest(body, protocol, route);
    const cipher = await CredentialsCipher.create(env.CREDENTIALS_AES_KEY);
    const abortController = new AbortController();
    if (request.signal.aborted) abortController.abort(request.signal.reason);
    else {
        request.signal.addEventListener(
            "abort",
            () => abortController.abort(request.signal.reason),
            {
                once: true,
            },
        );
    }
    const { prepared, response } = await executeUpstream(
        request,
        validated,
        protocol,
        route,
        env.DATABASE,
        cipher,
        abortController,
        fetchUpstream,
    );
    if (!response.ok) {
        if (prepared.isNativeProtocol) {
            return validated.isStreaming
                ? relayNativeStreamingResponse(response, abortController)
                : relayNativeNonStreaming(response, abortController);
        }
        return translatedUpstreamError(response, route, abortController);
    }
    if (prepared.isNativeProtocol) {
        return validated.isStreaming
            ? relayNativeStreamingResponse(response, abortController)
            : relayNativeNonStreaming(response, abortController);
    }
    if (validated.isStreaming) {
        return translateStreamingResponse(
            response,
            abortController,
            prepared.protocol,
            protocol,
            route.name,
            isUsageIncluded(body),
        );
    }
    const bytes = await readIdleTimedUpstreamBody(response, abortController);
    return renderTranslatedNonStreamingResponse(bytes, prepared.protocol, protocol, route.name);
}

export async function handleGatewayRequest(
    request: Request,
    env: Env,
    protocol: ClientProtocol,
    fetchUpstream: typeof fetch = fetch,
): Promise<Response> {
    try {
        return await handleGatewayChat(request, env, protocol, fetchUpstream);
    } catch (error) {
        if (error instanceof GatewayError) return gatewayErrorResponse(error);
        if (error instanceof TranslationError)
            return gatewayErrorResponse(mapTranslationError(error));
        if (error instanceof Error && error.message === "upstream idle timeout") {
            return gatewayErrorResponse(upstreamTimeoutError());
        }
        return gatewayErrorResponse(internalGatewayError());
    }
}
