import { ContractViolationError } from "../errors";
import type { CompleteProviderOauthResponse, StartProviderOauthResponse } from "../generated/v0";
import { expectRecord, expectSafeInteger, expectString } from "../validation-primitives";
import { parseProviderResponse } from "./providers";

/**
 * One begun flow: a union discriminated by the grant, because an
 * authorization-code flow answers with the URL to open and a device flow
 * answers with the verification facts to approve through.
 */
export function parseStartProviderOauthResponse(value: unknown): StartProviderOauthResponse {
    const record = expectRecord(value, "response");
    const grant = expectString(Reflect.get(record, "grant"), "response.grant");
    const state = expectString(Reflect.get(record, "state"), "response.state");
    const expiresAt = expectSafeInteger(Reflect.get(record, "expires_at"), "response.expires_at");

    if (grant === "authorization_code") {
        return {
            grant,
            state,
            expires_at: expiresAt,
            authorize_url: expectString(
                Reflect.get(record, "authorize_url"),
                "response.authorize_url",
            ),
        };
    }

    if (grant === "device") {
        const verificationUri = Reflect.get(record, "verification_uri");
        const verificationUriComplete = Reflect.get(record, "verification_uri_complete");

        return {
            grant,
            state,
            expires_at: expiresAt,
            user_code: expectString(Reflect.get(record, "user_code"), "response.user_code"),
            ...(verificationUri === undefined
                ? {}
                : {
                      verification_uri: expectString(verificationUri, "response.verification_uri"),
                  }),
            ...(verificationUriComplete === undefined
                ? {}
                : {
                      verification_uri_complete: expectString(
                          verificationUriComplete,
                          "response.verification_uri_complete",
                      ),
                  }),
        };
    }

    throw new ContractViolationError(`response.grant is not a known grant: ${grant}`);
}

/**
 * One finished or polled flow: succeeded carries the provider row, pending
 * carries nothing, failed carries the gateway-owned failure pair.
 */
export function parseCompleteProviderOauthResponse(value: unknown): CompleteProviderOauthResponse {
    const record = expectRecord(value, "response");
    const status = expectString(Reflect.get(record, "status"), "response.status");

    if (status === "succeeded") {
        return {
            status,
            provider: parseProviderResponse(Reflect.get(record, "provider"), "response.provider"),
        };
    }

    if (status === "pending") {
        return {
            status,
        };
    }

    if (status === "failed") {
        return {
            status,
            failure_code: expectString(
                Reflect.get(record, "failure_code"),
                "response.failure_code",
            ),
            failure_message: expectString(
                Reflect.get(record, "failure_message"),
                "response.failure_message",
            ),
        };
    }

    throw new ContractViolationError(`response.status is not a flow outcome: ${status}`);
}
