# Physical models

Generated from the Rust v0 route registry and serde-aware TypeScript bindings. Regenerate with `npm run docs:generate`; do not edit this page. Read the [management contract](../management-api.md) for transport, validation, defaults, and behavior.

## Routes

All paths below are complete. Success is HTTP 200 JSON.

| Method | Path | Request | Response |
| --- | --- | --- | --- |
| `POST` | `/management-api/v0/models:create` | [CreateModelRequest](#createmodelrequest) | [ModelEnvelope](#modelenvelope) |
| `POST` | `/management-api/v0/models:delete` | [DeleteModelRequest](#deletemodelrequest) | [EmptyResponse](common.md#emptyresponse) |
| `POST` | `/management-api/v0/models:discover` | [DiscoverModelsRequest](#discovermodelsrequest) | [DiscoverModelsResponse](#discovermodelsresponse) |
| `POST` | `/management-api/v0/models:list` | [ListModelsRequest](#listmodelsrequest) | [ListModelsResponse](#listmodelsresponse) |
| `POST` | `/management-api/v0/models:update` | [UpdateModelRequest](#updatemodelrequest) | [ModelEnvelope](#modelenvelope) |

## Payload schemas

Schemas use TypeScript notation for JSON objects and unions. `?` means omission is allowed; `null` is allowed only where written. `Record<symbol, never>` denotes the empty JSON object `{}`. Numeric bounds and accepted input defaults are specified in the owning contract.

### CreateModelRequest

```typescript
export type CreateModelRequest = {
    provider_id: number;
    upstream_id: string;
    is_enabled: boolean;
    thinking_levels_override: Array<string> | null;
    max_input_tokens_override: number | null;
    max_output_tokens_override: number | null;
    compaction_support_override: {
        [key: string]: "supported" | "unsupported" | "unknown";
    } | null;
};
```

### ModelEnvelope

```typescript
export type ModelEnvelope = {
    model: ModelResponse;
};
```

Related types: [ModelResponse](#modelresponse).

### ModelResponse

```typescript
export type ModelResponse = {
    id: number;
    provider_id: number;
    provider: string;
    upstream_id: string;
    is_enabled: boolean;
    discovery_state: DiscoveryState;
    thinking_levels: Array<string> | null;
    max_input_tokens: number | null;
    max_output_tokens: number | null;
    thinking_levels_override: Array<string> | null;
    max_input_tokens_override: number | null;
    max_output_tokens_override: number | null;
    compaction_support: {
        [key: string]: "supported" | "unsupported";
    } | null;
    compaction_support_override: {
        [key: string]: "supported" | "unsupported" | "unknown";
    } | null;
};
```

Related types: [DiscoveryState](#discoverystate).

### DiscoveryState

```typescript
export type DiscoveryState = "discovered" | "vanished" | "manual";
```

### DeleteModelRequest

```typescript
export type DeleteModelRequest = {
    model_id: number;
};
```

### DiscoverModelsRequest

```typescript
export type DiscoverModelsRequest = {
    provider_id: number;
};
```

### DiscoverModelsResponse

```typescript
export type DiscoverModelsResponse = {
    models: Array<ModelResponse>;
};
```

Related types: [ModelResponse](#modelresponse).

### ListModelsRequest

```typescript
export type ListModelsRequest = {
    provider_id?: number;
};
```

### ListModelsResponse

```typescript
export type ListModelsResponse = {
    models: Array<ModelResponse>;
};
```

Related types: [ModelResponse](#modelresponse).

### UpdateModelRequest

```typescript
export type UpdateModelRequest = {
    model_id: number;
    is_enabled: boolean;
    thinking_levels_override: Array<string> | null;
    max_input_tokens_override: number | null;
    max_output_tokens_override: number | null;
    compaction_support_override: {
        [key: string]: "supported" | "unsupported" | "unknown";
    } | null;
};
```

