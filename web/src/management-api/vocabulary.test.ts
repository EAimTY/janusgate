import { describe, expect, test } from "vitest";
import type { ProviderResponse } from "./generated/v0";
import {
    API_LOG_ENDPOINT_TYPE,
    API_LOG_OUTCOME,
    COMPACTION_MECHANISM,
    COMPACTION_MECHANISMS_BY_FORMAT,
    CREDENTIAL_KIND,
    DISCOVERY_STATE,
    isOAuthProviderRow,
    MECHANISM_SUPPORT,
    REASONING_LEVEL,
    UPSTREAM_FORMAT,
    USAGE_GROUP_BY,
} from "./vocabulary";

/**
 * The closed wire vocabularies: one options/label/find/parse surface per
 * vocabulary, where `find` answers null outside the set and `parse` names
 * the offending value with the vocabulary's noun; the compaction mechanisms
 * each upstream format can declare; and the OAuth row narrow the expiry
 * reads past.
 */

describe("vocabulary surface", () => {
    test("lists every member as an option whose value and label agree with the record", () => {
        expect(CREDENTIAL_KIND.values).toEqual([
            "api_key",
            "oauth",
        ]);
        expect(DISCOVERY_STATE.values).toEqual([
            "discovered",
            "vanished",
            "manual",
        ]);
        expect(REASONING_LEVEL.values).toEqual([
            "none",
            "minimal",
            "low",
            "medium",
            "high",
            "xhigh",
            "max",
        ]);

        // Options carry stable identities: every option is its own object.
        const [first, second] = CREDENTIAL_KIND.options;
        expect(CREDENTIAL_KIND.options[0]).toBe(first);
        expect(CREDENTIAL_KIND.options[1]).toBe(second);
    });

    test("labels a member and answers null for a value outside the set", () => {
        expect(DISCOVERY_STATE.label("vanished")).toBeTruthy();
        expect(DISCOVERY_STATE.find("vanished")).toBe("vanished");
        expect(DISCOVERY_STATE.find("exploded")).toBeNull();
        expect(UPSTREAM_FORMAT.find("openai_response")).toBe("openai_response");
        expect(UPSTREAM_FORMAT.find("grpc")).toBeNull();
    });

    test("names the offending value with the vocabulary's noun on a parse failure", () => {
        expect(() => MECHANISM_SUPPORT.parse("maybe", "row.compaction_support.x")).toThrowError(
            "row.compaction_support.x is not a mechanism support state: maybe",
        );
        expect(() => API_LOG_OUTCOME.parse("timed_out", "row.outcome")).toThrowError(
            "row.outcome is not a request outcome: timed_out",
        );
    });

    test("exposes every generated enum vocabulary in full", () => {
        expect(UPSTREAM_FORMAT.values).toEqual([
            "openai_response",
            "openai_chat_completion",
            "gemini_generate_content",
            "gemini_interaction",
            "anthropic_messages",
        ]);
        expect(MECHANISM_SUPPORT.values).toEqual([
            "supported",
            "unsupported",
            "unknown",
        ]);
        expect(API_LOG_ENDPOINT_TYPE.values).toEqual([
            "openai_chat_completions",
            "openai_responses",
            "openai_responses_compact",
            "gemini_generate_content",
            "gemini_interactions",
            "anthropic_messages",
        ]);
        expect(API_LOG_OUTCOME.values).toEqual([
            "completed",
            "client_aborted",
            "upstream_error",
            "internal",
        ]);
        expect(USAGE_GROUP_BY.values).toEqual([
            "day",
            "model",
            "model_route",
            "provider",
            "api_key",
            "endpoint_type",
            "outcome",
            "is_stream",
            "http_status",
        ]);
        expect(COMPACTION_MECHANISM.values).toEqual([
            "responses_standalone_compact",
            "responses_explicit_trigger",
            "responses_automatic_compaction",
            "messages_on_demand_compaction",
            "messages_threshold_compaction",
        ]);
    });
});

describe("COMPACTION_MECHANISMS_BY_FORMAT", () => {
    test("declares the Responses trio only on openai_response", () => {
        expect(COMPACTION_MECHANISMS_BY_FORMAT.openai_response).toEqual([
            "responses_standalone_compact",
            "responses_explicit_trigger",
            "responses_automatic_compaction",
        ]);
    });

    test("declares the Messages pair only on anthropic_messages, nothing on the others", () => {
        expect(COMPACTION_MECHANISMS_BY_FORMAT.anthropic_messages).toEqual([
            "messages_on_demand_compaction",
            "messages_threshold_compaction",
        ]);
        expect(COMPACTION_MECHANISMS_BY_FORMAT.openai_chat_completion).toEqual([]);
        expect(COMPACTION_MECHANISMS_BY_FORMAT.gemini_generate_content).toEqual([]);
        expect(COMPACTION_MECHANISMS_BY_FORMAT.gemini_interaction).toEqual([]);
    });
});

describe("isOAuthProviderRow", () => {
    test("narrows the OAuth member and rejects the static-key member", () => {
        const oauth: ProviderResponse = {
            type: "oauth",
            id: 1,
            name: "acme",
            model_prefix: "acme",
            is_enabled: true,
            provider_definition_name: "claude-code",
            provider_definition_version: 1,
            format: "anthropic_messages",
            base_url: null,
            expires_at: null,
        };
        const apiKey: ProviderResponse = {
            ...oauth,
            type: "api_key",
        };

        if (isOAuthProviderRow(oauth)) {
            // Past the guard the expiry is readable.
            expect(oauth.expires_at).toBeNull();
        } else {
            throw new Error("the OAuth row must narrow");
        }
        expect(isOAuthProviderRow(apiKey)).toBe(false);
    });
});
