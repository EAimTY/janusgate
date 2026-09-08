import type { ThinkingLevel } from "/config";

export const PROVIDER_TYPES = [
    "openai_oauth",
    "antigravity_oauth",
    "openai_compatible",
] as const;
export type ProviderType = (typeof PROVIDER_TYPES)[number];
/** Wire formats spoken by upstreams; clients speak every kind except `gemini_generate_content`. */
export const UPSTREAM_PROTOCOLS = [
    "openai_response",
    "openai_chat_completion",
    "gemini_generate_content",
] as const;
export type UpstreamProtocol = (typeof UPSTREAM_PROTOCOLS)[number];

const CLIENT_PROTOCOLS = [
    "openai_response",
    "openai_chat_completion",
] as const;
export type ClientProtocol = (typeof CLIENT_PROTOCOLS)[number];

export const OAUTH_FLOW_TYPES = [
    "openai",
    "antigravity",
] as const;
export type OAuthFlowType = (typeof OAUTH_FLOW_TYPES)[number];

export type OAuthProviderType = "openai_oauth" | "antigravity_oauth";

export function providerTypeForOAuthFlow(flowType: OAuthFlowType): OAuthProviderType {
    return flowType === "openai" ? "openai_oauth" : "antigravity_oauth";
}

export function oauthFlowForProviderType(providerType: ProviderType): OAuthFlowType | null {
    if (providerType === "openai_oauth") return "openai";
    if (providerType === "antigravity_oauth") return "antigravity";
    return null;
}

export interface OpenAiOAuthCredentials {
    readonly accessToken: string;
    readonly refreshToken: string;
    readonly accountId: string;
}

export interface AntigravityOAuthCredentials {
    readonly accessToken: string;
    readonly refreshToken: string;
    readonly projectId: string;
}

export interface OpenAiCompatibleCredentials {
    readonly apiKey: string;
}

export type ProviderCredentials =
    | OpenAiOAuthCredentials
    | AntigravityOAuthCredentials
    | OpenAiCompatibleCredentials;

interface ProviderSummaryBase {
    readonly name: string;
    readonly enabled: boolean;
    readonly modelPrefix: string;
}

interface OAuthProviderSummary extends ProviderSummaryBase {
    readonly type: OAuthProviderType;
    readonly protocol: null;
    readonly baseUrl: null;
    readonly expiresAt: number;
}

interface OpenAiCompatibleProviderSummary extends ProviderSummaryBase {
    readonly type: "openai_compatible";
    readonly protocol: UpstreamProtocol;
    readonly baseUrl: string;
    readonly expiresAt: null;
}

export type ProviderSummary = OAuthProviderSummary | OpenAiCompatibleProviderSummary;

/** Raw provider fields as stored in D1 or received on the panel wire; never trusted as a summary. */
interface ProviderSummaryFields {
    readonly name: string;
    readonly isEnabled: boolean;
    readonly modelPrefix: string;
    readonly type: ProviderType;
    readonly protocol: UpstreamProtocol | null;
    readonly baseUrl: string | null;
    readonly expiresAt: number | null;
}

/**
 * The single enforcement point of the `ProviderSummary` discriminated union (§12.3): an
 * OpenAI-compatible provider carries `protocol` and `baseUrl` and never a token expiry, and an
 * OAuth provider carries exactly the converse. Stored rows and panel responses both derive
 * through this one check, so the two sides cannot drift; a violation is a fatal contract error.
 */
export function assembleProviderSummary(fields: ProviderSummaryFields): ProviderSummary {
    const shared = {
        enabled: fields.isEnabled,
        modelPrefix: fields.modelPrefix,
        name: fields.name,
    };
    if (fields.type === "openai_compatible") {
        if (fields.protocol === null || fields.baseUrl === null || fields.expiresAt !== null) {
            throw new Error("Invalid OpenAI-compatible provider fields");
        }
        return {
            ...shared,
            baseUrl: fields.baseUrl,
            expiresAt: null,
            protocol: fields.protocol,
            type: fields.type,
        };
    }
    if (fields.protocol !== null || fields.baseUrl !== null || fields.expiresAt === null) {
        throw new Error("Invalid OAuth provider fields");
    }
    return {
        ...shared,
        baseUrl: null,
        expiresAt: fields.expiresAt,
        protocol: null,
        type: fields.type,
    };
}

export interface ModelSummary {
    readonly providerName: string;
    readonly providerType: ProviderType;
    readonly upstreamName: string;
    readonly name: string;
    readonly nameOverride: string | null;
    readonly enabled: boolean;
    readonly thinkingLevels: readonly ThinkingLevel[] | null;
}
