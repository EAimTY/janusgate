# Providers, quota, and availability

Generated from the Rust v0 route registry and serde-aware TypeScript bindings. Regenerate with `npm run docs:generate`; do not edit this page. Read the [management contract](../management-api.md) for transport, validation, defaults, and behavior.

## Routes

All paths below are complete. Success is HTTP 200 JSON.

| Method | Path | Request | Response |
| --- | --- | --- | --- |
| `POST` | `/management-api/v0/providers:availability` | [GetProviderAvailabilityRequest](#getprovideravailabilityrequest) | [GetProviderAvailabilityResponse](#getprovideravailabilityresponse) |
| `POST` | `/management-api/v0/providers:create` | [CreateProviderRequest](#createproviderrequest) | [ProviderEnvelope](#providerenvelope) |
| `POST` | `/management-api/v0/providers:delete` | [DeleteProviderRequest](#deleteproviderrequest) | [EmptyResponse](common.md#emptyresponse) |
| `POST` | `/management-api/v0/providers:list` | [ListProvidersRequest](#listprovidersrequest) | [ListProvidersResponse](#listprovidersresponse) |
| `POST` | `/management-api/v0/providers:quota` | [GetProviderQuotaRequest](#getproviderquotarequest) | [GetProviderQuotaResponse](#getproviderquotaresponse) |
| `POST` | `/management-api/v0/providers:reset-availability` | [ResetProviderAvailabilityRequest](#resetprovideravailabilityrequest) | [EmptyResponse](common.md#emptyresponse) |
| `POST` | `/management-api/v0/providers:update` | [UpdateProviderRequest](#updateproviderrequest) | [EmptyResponse](common.md#emptyresponse) |

## Payload schemas

Schemas use TypeScript notation for JSON objects and unions. `?` means omission is allowed; `null` is allowed only where written. `Record<symbol, never>` denotes the empty JSON object `{}`. Numeric bounds and accepted input defaults are specified in the owning contract.

### GetProviderAvailabilityRequest

```typescript
export type GetProviderAvailabilityRequest = {
    provider_id: number;
};
```

### GetProviderAvailabilityResponse

```typescript
export type GetProviderAvailabilityResponse = {
    blocks: Array<QuotaBlock>;
    last_refresh: LastRefresh;
};
```

Related types: [QuotaBlock](#quotablock), [LastRefresh](#lastrefresh).

### QuotaBlock

```typescript
export type QuotaBlock = {
    key: Array<string>;
    applies_to: QuotaApplicability;
    observed_at: number;
    blocked_until: number | null;
};
```

Related types: [QuotaApplicability](#quotaapplicability).

### QuotaApplicability

```typescript
export type QuotaApplicability =
    | {
          type: "all_models";
      }
    | {
          type: "models";
          upstream_ids: Array<string>;
      };
```

### LastRefresh

```typescript
export type LastRefresh =
    | {
          type: "not_observed";
      }
    | {
          type: "succeeded";
          attempted_at: SafeTimestamp;
          observed_at: SafeTimestamp;
      }
    | {
          type: "failed";
          attempted_at: SafeTimestamp;
          reason: RefreshFailure;
      }
    | {
          type: "unsupported";
          attempted_at: SafeTimestamp;
      }
    | {
          type: "reset";
          attempted_at: SafeTimestamp;
      };
```

Related types: [SafeTimestamp](#safetimestamp), [RefreshFailure](#refreshfailure).

### SafeTimestamp

```typescript
export type SafeTimestamp = number;
```

### RefreshFailure

```typescript
export type RefreshFailure =
    | "credential_renewal"
    | "timeout"
    | "transport"
    | "upstream_rejected"
    | "malformed_observation";
```

### CreateProviderRequest

```typescript
export type CreateProviderRequest = {
    model_prefix: string;
    provider_definition_name: string;
    provider_definition_version: number;
    name: string;
    is_enabled: boolean;
    base_url: string | null;
    api_key: string;
};
```

### ProviderEnvelope

```typescript
export type ProviderEnvelope = {
    provider: ProviderResponse;
};
```

Related types: [ProviderResponse](common.md#providerresponse).

### DeleteProviderRequest

```typescript
export type DeleteProviderRequest = {
    provider_id: number;
};
```

### ListProvidersRequest

```typescript
export type ListProvidersRequest = Record<symbol, never>;
```

### ListProvidersResponse

```typescript
export type ListProvidersResponse = {
    providers: Array<ProviderResponse>;
};
```

Related types: [ProviderResponse](common.md#providerresponse).

### GetProviderQuotaRequest

```typescript
export type GetProviderQuotaRequest = {
    provider_id: number;
};
```

### GetProviderQuotaResponse

```typescript
export type GetProviderQuotaResponse = {
    observed_at: SafeTimestamp;
    quotas: Array<QuotaEntry>;
};
```

Related types: [SafeTimestamp](#safetimestamp), [QuotaEntry](#quotaentry).

### QuotaEntry

```typescript
export type QuotaEntry = {
    key: Array<string>;
    label: string;
    scope: QuotaScope;
    routing: QuotaRouting;
    window: QuotaWindow;
    measurement: QuotaMeasurement;
    reset: QuotaReset;
};
```

Related types: [QuotaScope](#quotascope), [QuotaRouting](#quotarouting), [QuotaWindow](#quotawindow), [QuotaMeasurement](#quotameasurement), [QuotaReset](#quotareset).

### QuotaScope

```typescript
export type QuotaScope =
    | {
          type: "unknown";
      }
    | {
          type: "named";
          name: string;
      };
```

### QuotaRouting

```typescript
export type QuotaRouting =
    | {
          type: "monitoring_only";
      }
    | {
          type: "all_models";
      }
    | {
          type: "models";
          upstream_ids: Array<string>;
      };
```

### QuotaWindow

```typescript
export type QuotaWindow =
    | {
          type: "unknown";
      }
    | {
          type: "duration";
          seconds: SafeDuration;
      }
    | {
          type: "calendar";
          unit: QuotaCalendarUnit;
      };
```

Related types: [SafeDuration](common.md#safeduration), [QuotaCalendarUnit](common.md#quotacalendarunit).

### QuotaMeasurement

```typescript
export type QuotaMeasurement =
    | {
          type: "unknown";
      }
    | {
          type: "ratio";
          used_fraction: Amount;
      }
    | {
          type: "counter";
          used: Amount;
          limit: PositiveAmount;
          unit: string;
      }
    | {
          type: "remaining";
          remaining: Amount;
          unit: string;
      }
    | {
          type: "unlimited";
      };
```

Related types: [Amount](#amount), [PositiveAmount](#positiveamount).

### Amount

```typescript
export type Amount = number;
```

### PositiveAmount

```typescript
export type PositiveAmount = number;
```

### QuotaReset

```typescript
export type QuotaReset =
    | {
          type: "unknown";
      }
    | {
          type: "at";
          at: SafeTimestamp;
      }
    | {
          type: "date";
          date: string;
      };
```

Related types: [SafeTimestamp](#safetimestamp).

### ResetProviderAvailabilityRequest

```typescript
export type ResetProviderAvailabilityRequest = {
    provider_id: number;
};
```

### UpdateProviderRequest

```typescript
export type UpdateProviderRequest = {
    model_prefix: string;
    provider_id: number;
    name: string;
    is_enabled: boolean;
    base_url: string | null;
    provider_definition_name?: string;
    provider_definition_version?: number;
    api_key?: string;
};
```

