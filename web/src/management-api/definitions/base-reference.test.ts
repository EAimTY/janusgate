import { describe, expect, it } from "vitest";
import type { ControlRequest, ProvisioningStep } from "../generated/v0";
import { hasBaseReference } from "./base-reference";
import type { DefinitionPayload } from "./types";

/** One minimal base-free payload, carrying exactly the provisioning step given. */
function payloadWithProvisioningStep(step: ProvisioningStep[] | null): DefinitionPayload {
    return {
        format: "openai_chat_completion",
        inference: {
            url: [
                {
                    type: "literal",
                    value: "https://upstream.example.com/v1/chat/completions",
                },
            ],
            upstream_streaming: "match_client",
        },
        credential: {
            type: "api_key",
        },
        provisioning: step,
    };
}

/** One provisioning step whose own request is base-free and whose poll carries the given request. */
function stepWithPollRequest(pollRequest: ControlRequest): ProvisioningStep {
    return {
        request: {
            method: "POST",
            url: [
                {
                    type: "literal",
                    value: "https://operations.example.com/start",
                },
            ],
        },
        response_encoding: "json",
        poll: {
            request: pollRequest,
            response_encoding: "json",
            done_path: [
                "done",
            ],
            error_path: [
                "error",
            ],
            interval_ms: 1_000,
            budget_ms: 60_000,
        },
    };
}

/** One base-free poll request whose body carries the given control body. */
function pollRequestWithBody(body: Exclude<ControlRequest["body"], undefined>): ControlRequest {
    return {
        method: "POST",
        url: [
            {
                type: "literal",
                value: "https://operations.example.com/poll",
            },
        ],
        body,
    };
}

describe("hasBaseReference", () => {
    it("reads a provisioning poll's full control surface, its request body included", () => {
        // The poll's JSON body string leaf references the base — the surface
        // the gateway's `base_capable_surfaces` covers whole.
        const jsonBody = stepWithPollRequest(
            pollRequestWithBody({
                kind: "json",
                node: {
                    type: "string",
                    expr: [
                        {
                            type: "base",
                        },
                    ],
                },
            }),
        );
        expect(
            hasBaseReference(
                payloadWithProvisioningStep([
                    jsonBody,
                ]),
            ),
        ).toBe(true);

        // The poll's form field values are body surface too.
        const formBody = stepWithPollRequest(
            pollRequestWithBody({
                kind: "form",
                fields: [
                    {
                        name: "address",
                        value: [
                            {
                                type: "base",
                            },
                        ],
                    },
                ],
            }),
        );
        expect(
            hasBaseReference(
                payloadWithProvisioningStep([
                    formBody,
                ]),
            ),
        ).toBe(true);
    });

    it("keeps a base-free poll base-free, whatever its other members carry", () => {
        const literal = stepWithPollRequest(
            pollRequestWithBody({
                kind: "json",
                node: {
                    type: "string",
                    expr: [
                        {
                            type: "slot",
                            name: "project",
                        },
                    ],
                },
            }),
        );
        expect(
            hasBaseReference(
                payloadWithProvisioningStep([
                    literal,
                ]),
            ),
        ).toBe(false);
        expect(hasBaseReference(payloadWithProvisioningStep(null))).toBe(false);
    });
});
