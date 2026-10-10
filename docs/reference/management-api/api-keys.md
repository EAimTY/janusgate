# Serving keys and model access

Generated from the Rust v0 route registry and serde-aware TypeScript bindings. Regenerate with `npm run docs:generate`; do not edit this page. Read the [management contract](../management-api.md) for transport, validation, defaults, and behavior.

## Routes

All paths below are complete. Success is HTTP 200 JSON.

| Method | Path | Request | Response |
| --- | --- | --- | --- |
| `POST` | `/management-api/v0/api-keys:create` | [CreateApiKeyRequest](#createapikeyrequest) | [ApiKeyEnvelope](#apikeyenvelope) |
| `POST` | `/management-api/v0/api-keys:delete` | [DeleteApiKeyRequest](#deleteapikeyrequest) | [EmptyResponse](common.md#emptyresponse) |
| `POST` | `/management-api/v0/api-keys:list` | [ListApiKeysRequest](#listapikeysrequest) | [ListApiKeysResponse](#listapikeysresponse) |
| `POST` | `/management-api/v0/api-keys:update` | [UpdateApiKeyRequest](#updateapikeyrequest) | [EmptyResponse](common.md#emptyresponse) |

## Payload schemas

Schemas use TypeScript notation for JSON objects and unions. `?` means omission is allowed; `null` is allowed only where written. `Record<symbol, never>` denotes the empty JSON object `{}`. Numeric bounds and accepted input defaults are specified in the owning contract.

### CreateApiKeyRequest

```typescript
export type CreateApiKeyRequest = {
    name: string;
    key: string;
    is_enabled: boolean;
    access_control: ApiKeyAccessControl;
};
```

Related types: [ApiKeyAccessControl](#apikeyaccesscontrol).

### ApiKeyAccessControl

```typescript
export type ApiKeyAccessControl = {
    default_access: AccessDecision;
    owner_defaults: Array<ApiKeyOwnerDefault>;
    route_overrides: Array<ApiKeyRouteOverride>;
};
```

Related types: [AccessDecision](#accessdecision), [ApiKeyOwnerDefault](#apikeyownerdefault), [ApiKeyRouteOverride](#apikeyrouteoverride).

### AccessDecision

```typescript
export type AccessDecision = "allow" | "deny";
```

### ApiKeyOwnerDefault

```typescript
export type ApiKeyOwnerDefault =
    | {
          type: "provider";
          provider_id: number;
          access: AccessDecision;
      }
    | {
          type: "pool";
          pool_id: number;
          access: AccessDecision;
      };
```

Related types: [AccessDecision](#accessdecision).

### ApiKeyRouteOverride

```typescript
export type ApiKeyRouteOverride = {
    model_route_id: number;
    access: AccessDecision;
};
```

Related types: [AccessDecision](#accessdecision).

### ApiKeyEnvelope

```typescript
export type ApiKeyEnvelope = {
    api_key: ApiKeyResponse;
};
```

Related types: [ApiKeyResponse](#apikeyresponse).

### ApiKeyResponse

```typescript
export type ApiKeyResponse = {
    id: number;
    name: string;
    key: string;
    is_enabled: boolean;
    access_control: ApiKeyAccessControl;
    created_at: number;
};
```

Related types: [ApiKeyAccessControl](#apikeyaccesscontrol).

### DeleteApiKeyRequest

```typescript
export type DeleteApiKeyRequest = {
    api_key_id: number;
};
```

### ListApiKeysRequest

```typescript
export type ListApiKeysRequest = Record<symbol, never>;
```

### ListApiKeysResponse

```typescript
export type ListApiKeysResponse = {
    api_keys: Array<ApiKeyResponse>;
};
```

Related types: [ApiKeyResponse](#apikeyresponse).

### UpdateApiKeyRequest

```typescript
export type UpdateApiKeyRequest = {
    api_key_id: number;
    name: string;
    key: string;
    is_enabled: boolean;
    access_control: ApiKeyAccessControl;
};
```

Related types: [ApiKeyAccessControl](#apikeyaccesscontrol).

