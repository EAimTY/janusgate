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

interface StoredModel {
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

/**
 * The single gate between a raw `providers.type` column and the `ProviderType` union: D1 has no
 * CHECK constraint, so a corrupted row fails loudly here instead of leaking an invalid type.
 */
export function parseStoredProviderType(value: string): ProviderType {
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
        type: parseStoredProviderType(row.type),
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

/**
 * The single owner of the column-list splice rule: names are comma-joined in order, optionally
 * qualified by a table name, and optionally aliased where a joined row reuses a column name.
 * Every SELECT projection and INSERT target list derives from this so lists cannot drift apart
 * or from the canonical schema-order tuples below.
 */
export function columnList(
    names: readonly string[],
    options: {
        /** Appends `AS alias` to the matching column, e.g. `{ name: "model_name" }`. */
        aliases?: Readonly<Record<string, string>>;
        /** Renders each name as `table.name` when present. */
        table?: string;
    } = {},
): string {
    return names
        .map((name) => {
            const reference = options.table === undefined ? name : `${options.table}.${name}`;
            const alias = options.aliases?.[name];
            return alias === undefined ? reference : `${reference} AS ${alias}`;
        })
        .join(", ");
}

/**
 * Canonical provider column names in schema order. Every provider SELECT must derive its
 * column list from this, so a schema change updates all queries together instead of
 * leaving hand-written lists behind.
 */
export const PROVIDER_COLUMN_NAMES = [
    "id",
    "name",
    "model_prefix",
    "type",
    "protocol",
    "base_url",
    "encrypted_credentials",
    "expires_at",
    "refresh_started_at",
    "enabled",
] as const;

export const PROVIDER_COLUMNS = columnList(PROVIDER_COLUMN_NAMES);

/**
 * Canonical model column names in schema order. Every models-table column list must derive from
 * this, mirroring PROVIDER_COLUMN_NAMES, so a schema change updates all queries together instead
 * of leaving hand-written lists behind.
 */
export const MODEL_COLUMN_NAMES = [
    "provider_id",
    "upstream_name",
    "name",
    "name_override",
    "enabled_override",
    "thinking_levels_override",
] as const;

/**
 * The single provider lookup shared by every read-by-one call site: selects exactly the canonical
 * PROVIDER_COLUMNS and maps through mapProviderRow, so a row is validated in exactly one place.
 * The overloads pin each value type to its lookup column, and the column interpolates only from
 * that compile-time union — never from arbitrary input.
 */
export function findProviderRow(
    database: D1Database,
    column: "id",
    value: number,
): Promise<StoredProvider | null>;
export function findProviderRow(
    database: D1Database,
    column: "name",
    value: string,
): Promise<StoredProvider | null>;
export async function findProviderRow(
    database: D1Database,
    column: "id" | "name",
    value: number | string,
): Promise<StoredProvider | null> {
    const row = await database
        .prepare(`SELECT ${PROVIDER_COLUMNS} FROM providers WHERE providers.${column} = ?`)
        .bind(value)
        .first<ProviderRow>();
    return row === null ? null : mapProviderRow(row);
}
