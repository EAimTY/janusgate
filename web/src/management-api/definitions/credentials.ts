import { ContractViolationError } from "../errors";
import type {
    CredentialAcquisition,
    Exchange,
    Grant,
    Poll,
    ProvisionExtraction,
    ProvisioningStep,
    TokenRefresh,
} from "../generated/v0";
import {
    expectArray,
    expectBoolean,
    expectDeclaredKeys,
    expectInteger,
    expectOneOf,
    expectRecord,
    expectString,
    isPresentValue,
} from "../validation-primitives";
import {
    expectControlRequest,
    expectExtractionShape,
    expectResponseDescriptor,
    expectResponseEncoding,
    expectResponsePath,
    expectResponseTarget,
} from "./control";
import { expectParamRows, expectStringExpr } from "./expressions";

/** One provisioning extraction: the path, its target, the shape, and the flag. */
function expectProvisionExtraction(value: unknown, context: string): ProvisionExtraction {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "path",
            "target",
            "shape",
            "required",
        ],
        context,
    );

    return {
        path: expectResponsePath(Reflect.get(record, "path"), `${context}.path`),
        target: expectResponseTarget(Reflect.get(record, "target"), `${context}.target`),
        shape: expectExtractionShape(Reflect.get(record, "shape"), `${context}.shape`),
        required: expectBoolean(Reflect.get(record, "required"), `${context}.required`),
    };
}

/** The extractions of one provisioning step or poll, when the key is present. */
function expectProvisionExtractions(value: unknown, context: string): Array<ProvisionExtraction> {
    return expectArray(value, context).map((entry, index) =>
        expectProvisionExtraction(entry, `${context}[${index}]`),
    );
}

/** The terminal status codes of an exchange or a grant-mode token refresh. */
function expectTerminalCodes(value: unknown, context: string): Array<string> {
    return expectArray(value, context).map((code, index) =>
        expectString(code, `${context}[${index}]`),
    );
}

/** The long-running operation poll of a provisioning step (README §5.4). */
function expectPoll(value: unknown, context: string): Poll {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "request",
            "response_encoding",
            "done_path",
            "error_path",
            "result_extract",
            "interval_ms",
            "budget_ms",
        ],
        context,
    );

    const poll: Poll = {
        request: expectControlRequest(Reflect.get(record, "request"), `${context}.request`),
        response_encoding: expectResponseEncoding(
            Reflect.get(record, "response_encoding"),
            `${context}.response_encoding`,
        ),
        done_path: expectResponsePath(Reflect.get(record, "done_path"), `${context}.done_path`),
        error_path: expectResponsePath(Reflect.get(record, "error_path"), `${context}.error_path`),
        interval_ms: expectInteger(Reflect.get(record, "interval_ms"), `${context}.interval_ms`),
        budget_ms: expectInteger(Reflect.get(record, "budget_ms"), `${context}.budget_ms`),
    };

    const resultExtract = Reflect.get(record, "result_extract");

    if (resultExtract !== undefined) {
        poll.result_extract = expectProvisionExtractions(
            resultExtract,
            `${context}.result_extract`,
        );
    }

    return poll;
}

/** One provisioning step, rebuilt with its guard, extractions, and poll (README §5.4). */
function expectProvisioningStep(value: unknown, context: string): ProvisioningStep {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "request",
            "response_encoding",
            "extract",
            "skip_if",
            "poll",
        ],
        context,
    );

    const step: ProvisioningStep = {
        request: expectControlRequest(Reflect.get(record, "request"), `${context}.request`),
        response_encoding: expectResponseEncoding(
            Reflect.get(record, "response_encoding"),
            `${context}.response_encoding`,
        ),
    };

    const extract = Reflect.get(record, "extract");

    if (extract !== undefined) {
        step.extract = expectProvisionExtractions(extract, `${context}.extract`);
    }

    const skipIf = Reflect.get(record, "skip_if");

    if (isPresentValue(skipIf)) {
        step.skip_if = expectString(skipIf, `${context}.skip_if`);
    }

    const poll = Reflect.get(record, "poll");

    if (isPresentValue(poll)) {
        step.poll = expectPoll(poll, `${context}.poll`);
    }

    return step;
}

/** Validates one provisioning sequence with its polls (README §5.4). */
export function expectProvisioning(value: unknown, context: string): Array<ProvisioningStep> {
    return expectArray(value, context).map((step, index) =>
        expectProvisioningStep(step, `${context}[${index}]`),
    );
}

/** One durable slot initializer, rebuilt (README §5.3). */
function expectSlotInitializer(
    value: unknown,
    context: string,
): {
    name: string;
    generator: "uuid" | "uuid_hex";
} {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "name",
            "generator",
        ],
        context,
    );
    const generator = expectOneOf(
        expectString(Reflect.get(record, "generator"), `${context}.generator`),
        [
            "uuid",
            "uuid_hex",
        ],
        `${context}.generator`,
        "a slot generator",
    );

    return {
        name: expectString(Reflect.get(record, "name"), `${context}.name`),
        generator,
    };
}

/** The OAuth authorization grant a flow runs, rebuilt from its kind (README §5.3). */
function expectGrant(value: unknown, context: string): Grant {
    const record = expectRecord(value, context);
    const type = expectString(Reflect.get(record, "type"), `${context}.type`);

    if (type === "authorization_code") {
        expectDeclaredKeys(
            record,
            [
                "type",
                "authorize_url",
                "redirect_uri",
                "pkce",
                "extra_params",
            ],
            context,
        );

        const pkce = expectOneOf(
            expectString(Reflect.get(record, "pkce"), `${context}.pkce`),
            [
                "none",
                "s256",
            ],
            `${context}.pkce`,
            "a PKCE mode",
        );

        const grant: Grant = {
            type: "authorization_code",
            authorize_url: expectStringExpr(
                Reflect.get(record, "authorize_url"),
                `${context}.authorize_url`,
            ),
            redirect_uri: expectString(
                Reflect.get(record, "redirect_uri"),
                `${context}.redirect_uri`,
            ),
            pkce,
        };

        const extraParams = Reflect.get(record, "extra_params");

        if (extraParams !== undefined) {
            grant.extra_params = expectParamRows(extraParams, `${context}.extra_params`);
        }

        return grant;
    }

    if (type === "device") {
        expectDeclaredKeys(
            record,
            [
                "type",
                "authorization",
                "expires_in_fallback_seconds",
            ],
            context,
        );

        const authorizationRecord = expectRecord(
            Reflect.get(record, "authorization"),
            `${context}.authorization`,
        );
        expectDeclaredKeys(
            authorizationRecord,
            [
                "request",
                "response",
            ],
            `${context}.authorization`,
        );

        const grant: Grant = {
            type: "device",
            authorization: {
                request: expectControlRequest(
                    Reflect.get(authorizationRecord, "request"),
                    `${context}.authorization.request`,
                ),
                response: expectResponseDescriptor(
                    Reflect.get(authorizationRecord, "response"),
                    `${context}.authorization.response`,
                ),
            },
        };

        const fallback = Reflect.get(record, "expires_in_fallback_seconds");

        if (isPresentValue(fallback)) {
            grant.expires_in_fallback_seconds = expectInteger(
                fallback,
                `${context}.expires_in_fallback_seconds`,
            );
        }

        return grant;
    }

    throw new ContractViolationError(`${context}.type is not a grant kind: ${type}`);
}

/** The token exchange redeeming a grant's product (README §5.3). */
function expectExchange(value: unknown, context: string): Exchange {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "request",
            "response",
            "terminal_codes",
        ],
        context,
    );

    return {
        request: expectControlRequest(Reflect.get(record, "request"), `${context}.request`),
        response: expectResponseDescriptor(Reflect.get(record, "response"), `${context}.response`),
        terminal_codes: expectTerminalCodes(
            Reflect.get(record, "terminal_codes"),
            `${context}.terminal_codes`,
        ),
    };
}

/** How an OAuth-acquired credential renews, rebuilt from its mode (README §5.3). */
function expectTokenRefresh(value: unknown, context: string): TokenRefresh {
    const record = expectRecord(value, context);
    const type = expectString(Reflect.get(record, "type"), `${context}.type`);

    if (type === "grant") {
        expectDeclaredKeys(
            record,
            [
                "type",
                "request",
                "response",
                "terminal_codes",
            ],
            context,
        );

        const refresh: Extract<
            TokenRefresh,
            {
                type: "grant";
            }
        > = {
            type: "grant",
            request: expectControlRequest(Reflect.get(record, "request"), `${context}.request`),
            terminal_codes: expectTerminalCodes(
                Reflect.get(record, "terminal_codes"),
                `${context}.terminal_codes`,
            ),
        };

        const response = Reflect.get(record, "response");

        if (isPresentValue(response)) {
            refresh.response = expectResponseDescriptor(response, `${context}.response`);
        }

        return refresh;
    }

    if (type === "reprovision" || type === "none") {
        expectDeclaredKeys(
            record,
            [
                "type",
            ],
            context,
        );
        return {
            type,
        };
    }

    throw new ContractViolationError(`${context}.type is not a refresh mode: ${type}`);
}

/**
 * The credential acquisition of a definition, rebuilt from its kind
 * (README §5.3): an `api_key` credential is its tag alone, an `oauth`
 * credential the full recipe.
 */
export function expectCredential(value: unknown, context: string): CredentialAcquisition {
    const record = expectRecord(value, context);
    const kind = expectString(Reflect.get(record, "type"), `${context}.type`);

    if (kind === "api_key") {
        expectDeclaredKeys(
            record,
            [
                "type",
            ],
            context,
        );
        return {
            type: "api_key",
        };
    }

    if (kind === "oauth") {
        expectDeclaredKeys(
            record,
            [
                "type",
                "client_id",
                "client_secret",
                "access_token_lifetime_seconds",
                "initial_slots",
                "grant",
                "exchange",
                "id_token_claims",
                "refresh",
            ],
            context,
        );

        const oauth: Extract<
            CredentialAcquisition,
            {
                type: "oauth";
            }
        > = {
            type: "oauth",
            client_id: expectString(Reflect.get(record, "client_id"), `${context}.client_id`),
            grant: expectGrant(Reflect.get(record, "grant"), `${context}.grant`),
            exchange: expectExchange(Reflect.get(record, "exchange"), `${context}.exchange`),
            refresh: expectTokenRefresh(Reflect.get(record, "refresh"), `${context}.refresh`),
        };

        const clientSecret = Reflect.get(record, "client_secret");

        if (isPresentValue(clientSecret)) {
            oauth.client_secret = expectString(clientSecret, `${context}.client_secret`);
        }

        const lifetime = Reflect.get(record, "access_token_lifetime_seconds");

        if (isPresentValue(lifetime)) {
            oauth.access_token_lifetime_seconds = expectInteger(
                lifetime,
                `${context}.access_token_lifetime_seconds`,
            );
        }

        const initialSlots = Reflect.get(record, "initial_slots");

        if (initialSlots !== undefined) {
            oauth.initial_slots = expectArray(initialSlots, `${context}.initial_slots`).map(
                (slot, index) => expectSlotInitializer(slot, `${context}.initial_slots[${index}]`),
            );
        }

        const idTokenClaims = Reflect.get(record, "id_token_claims");

        if (idTokenClaims !== undefined) {
            oauth.id_token_claims = expectArray(idTokenClaims, `${context}.id_token_claims`).map(
                (claim, index) => {
                    const claimRecord = expectRecord(claim, `${context}.id_token_claims[${index}]`);
                    expectDeclaredKeys(
                        claimRecord,
                        [
                            "claim",
                            "slot",
                        ],
                        `${context}.id_token_claims[${index}]`,
                    );

                    return {
                        claim: expectResponsePath(
                            Reflect.get(claimRecord, "claim"),
                            `${context}.id_token_claims[${index}].claim`,
                        ),
                        slot: expectString(
                            Reflect.get(claimRecord, "slot"),
                            `${context}.id_token_claims[${index}].slot`,
                        ),
                    };
                },
            );
        }

        return oauth;
    }

    throw new ContractViolationError(`${context}.type is not a credential kind: ${kind}`);
}
