import {
    PROVIDER_TYPES,
    type ProviderType,
    UPSTREAM_PROTOCOLS,
    type UpstreamProtocol,
} from "/common/domain";

export interface StoredProvider {
    id: number;
    name: string;
    modelPrefix: string;
    type: ProviderType;
    protocol: UpstreamProtocol | null;
    baseUrl: string | null;
    encryptedCredentials: ArrayBuffer;
    expiresAt: number | null;
    refreshStartedAt: number | null;
    isEnabled: boolean;
}

export interface StoredModel {
    id: number;
    name: string;
    providerId: number;
    upstreamName: string;
    thinkingLevelsJson: string | null;
    isEnabled: boolean;
    isManuallyEdited: boolean;
}

export interface ProviderRow {
    id: number;
    name: string;
    model_prefix: string;
    type: string;
    protocol: string | null;
    base_url: string | null;
    encrypted_credentials: ArrayBuffer;
    expires_at: number | null;
    refresh_started_at: number | null;
    enabled: number;
}

export interface ModelRow {
    id: number;
    name: string;
    provider_id: number;
    upstream_name: string;
    thinking_levels: string | null;
    enabled: number;
    is_manually_edited: number;
}

function parseProviderType(value: string): ProviderType {
    for (const providerType of PROVIDER_TYPES) {
        if (value === providerType) return providerType;
    }
    throw new Error(`Invalid stored provider type: ${value}`);
}

function parseProtocol(value: string | null): UpstreamProtocol | null {
    if (value === null) return null;
    for (const protocol of UPSTREAM_PROTOCOLS) {
        if (value === protocol) return protocol;
    }
    throw new Error(`Invalid stored provider protocol: ${value}`);
}

function parseStoredBoolean(value: number): boolean {
    if (value === 0) return false;
    if (value === 1) return true;
    throw new Error(`Invalid stored boolean: ${value}`);
}

export function mapProviderRow(row: ProviderRow): StoredProvider {
    return {
        baseUrl: row.base_url,
        encryptedCredentials: row.encrypted_credentials,
        expiresAt: row.expires_at,
        id: row.id,
        isEnabled: parseStoredBoolean(row.enabled),
        modelPrefix: row.model_prefix,
        name: row.name,
        protocol: parseProtocol(row.protocol),
        refreshStartedAt: row.refresh_started_at,
        type: parseProviderType(row.type),
    };
}

export function mapModelRow(row: ModelRow): StoredModel {
    return {
        id: row.id,
        isEnabled: parseStoredBoolean(row.enabled),
        isManuallyEdited: parseStoredBoolean(row.is_manually_edited),
        name: row.name,
        providerId: row.provider_id,
        thinkingLevelsJson: row.thinking_levels,
        upstreamName: row.upstream_name,
    };
}

export const PROVIDER_COLUMNS = `
    id,
    name,
    model_prefix,
    type,
    protocol,
    base_url,
    encrypted_credentials,
    expires_at,
    refresh_started_at,
    enabled
`;

export const MODEL_COLUMNS = `
    id,
    name,
    provider_id,
    upstream_name,
    thinking_levels,
    enabled,
    is_manually_edited
`;
