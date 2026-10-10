# Provider pools

Generated from the Rust v0 route registry and serde-aware TypeScript bindings. Regenerate with `npm run docs:generate`; do not edit this page. Read the [management contract](../management-api.md) for transport, validation, defaults, and behavior.

## Routes

All paths below are complete. Success is HTTP 200 JSON.

| Method | Path | Request | Response |
| --- | --- | --- | --- |
| `POST` | `/management-api/v0/provider-pools:create` | [CreateProviderPoolRequest](#createproviderpoolrequest) | [CreateProviderPoolResponse](#createproviderpoolresponse) |
| `POST` | `/management-api/v0/provider-pools:delete` | [DeleteProviderPoolRequest](#deleteproviderpoolrequest) | [EmptyResponse](common.md#emptyresponse) |
| `POST` | `/management-api/v0/provider-pools:list` | [ListProviderPoolsRequest](#listproviderpoolsrequest) | [ListProviderPoolsResponse](#listproviderpoolsresponse) |
| `POST` | `/management-api/v0/provider-pools:update` | [UpdateProviderPoolRequest](#updateproviderpoolrequest) | [EmptyResponse](common.md#emptyresponse) |

## Payload schemas

Schemas use TypeScript notation for JSON objects and unions. `?` means omission is allowed; `null` is allowed only where written. `Record<symbol, never>` denotes the empty JSON object `{}`. Numeric bounds and accepted input defaults are specified in the owning contract.

### CreateProviderPoolRequest

```typescript
export type CreateProviderPoolRequest = {
    name: string;
    model_prefix: string;
    is_enabled: boolean;
    members: Array<ProviderPoolMember>;
};
```

Related types: [ProviderPoolMember](#providerpoolmember).

### ProviderPoolMember

```typescript
export type ProviderPoolMember = {
    provider_id: number;
    priority: number;
};
```

### CreateProviderPoolResponse

```typescript
export type CreateProviderPoolResponse = {
    provider_pool_id: number;
};
```

### DeleteProviderPoolRequest

```typescript
export type DeleteProviderPoolRequest = {
    provider_pool_id: number;
};
```

### ListProviderPoolsRequest

```typescript
export type ListProviderPoolsRequest = Record<symbol, never>;
```

### ListProviderPoolsResponse

```typescript
export type ListProviderPoolsResponse = {
    provider_pools: Array<ProviderPoolResponse>;
};
```

Related types: [ProviderPoolResponse](#providerpoolresponse).

### ProviderPoolResponse

```typescript
export type ProviderPoolResponse = {
    id: number;
    name: string;
    model_prefix: string;
    is_enabled: boolean;
    members: Array<ProviderPoolMember>;
};
```

Related types: [ProviderPoolMember](#providerpoolmember).

### UpdateProviderPoolRequest

```typescript
export type UpdateProviderPoolRequest = {
    provider_pool_id: number;
    name: string;
    model_prefix: string;
    is_enabled: boolean;
    members: Array<ProviderPoolMember>;
};
```

Related types: [ProviderPoolMember](#providerpoolmember).

