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
    providerId: number;
    upstreamName: string;
    name: string;
    nameOverride: string | null;
    enabledOverride: boolean | null;
    thinkingLevelsOverrideJson: string | null;
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
    provider_id: number;
    upstream_name: string;
    name: string;
    name_override: string | null;
    enabled_override: number | null;
    thinking_levels_override: string | null;
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
        enabledOverride:
            row.enabled_override === null ? null : parseStoredBoolean(row.enabled_override),
        name: row.name,
        nameOverride: row.name_override,
        providerId: row.provider_id,
        thinkingLevelsOverrideJson: row.thinking_levels_override,
        upstreamName: row.upstream_name,
    };
}

/** Effective serving state: the override wins when present, else the discovery default (enabled). */
export function isModelEnabled(model: StoredModel): boolean {
    return model.enabledOverride !== false;
}

/** True iff any discovery-derived field is overridden, which shields the row from discovery pruning. */
export function hasModelOverrides(model: StoredModel): boolean {
    return (
        model.nameOverride !== null ||
        model.enabledOverride !== null ||
        model.thinkingLevelsOverrideJson !== null
    );
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
    provider_id,
    upstream_name,
    name,
    name_override,
    enabled_override,
    thinking_levels_override
`;
