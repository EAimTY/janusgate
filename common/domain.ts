import type { ThinkingLevel } from "/config";

export const PROVIDER_TYPES = [
    "openai_oauth",
    "antigravity_oauth",
    "openai_compatible",
] as const;
export type ProviderType = (typeof PROVIDER_TYPES)[number];
export const UPSTREAM_PROTOCOLS = [
    "responses",
    "chat_completions",
] as const;
export type UpstreamProtocol = (typeof UPSTREAM_PROTOCOLS)[number];

export const OAUTH_FLOW_TYPES = [
    "openai",
    "antigravity",
] as const;
export type OAuthFlowType = (typeof OAUTH_FLOW_TYPES)[number];

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

export interface OAuthProviderSummary extends ProviderSummaryBase {
    readonly type: "antigravity_oauth" | "openai_oauth";
    readonly protocol: null;
    readonly baseUrl: null;
    readonly expiresAt: number;
}

export interface OpenAiCompatibleProviderSummary extends ProviderSummaryBase {
    readonly type: "openai_compatible";
    readonly protocol: UpstreamProtocol;
    readonly baseUrl: string;
    readonly expiresAt: null;
}

export type ProviderSummary = OAuthProviderSummary | OpenAiCompatibleProviderSummary;

export interface ModelSummary {
    readonly id: number;
    readonly name: string;
    readonly providerName: string;
    readonly providerType: ProviderType;
    readonly upstreamName: string;
    readonly enabled: boolean;
    readonly thinkingLevels: readonly ThinkingLevel[] | null;
}

export interface DiscoveryResult {
    readonly total: number;
    readonly created: number;
    readonly skipped: number;
}
