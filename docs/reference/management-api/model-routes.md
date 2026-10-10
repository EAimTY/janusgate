# Public model routes

Generated from the Rust v0 route registry and serde-aware TypeScript bindings. Regenerate with `npm run docs:generate`; do not edit this page. Read the [management contract](../management-api.md) for transport, validation, defaults, and behavior.

## Routes

All paths below are complete. Success is HTTP 200 JSON.

| Method | Path | Request | Response |
| --- | --- | --- | --- |
| `POST` | `/management-api/v0/model-routes:create` | [CreateModelRouteRequest](#createmodelrouterequest) | [CreateModelRouteResponse](#createmodelrouteresponse) |
| `POST` | `/management-api/v0/model-routes:delete` | [DeleteModelRouteRequest](#deletemodelrouterequest) | [EmptyResponse](common.md#emptyresponse) |
| `POST` | `/management-api/v0/model-routes:list` | [ListModelRoutesRequest](#listmodelroutesrequest) | [ListModelRoutesResponse](#listmodelroutesresponse) |
| `POST` | `/management-api/v0/model-routes:update` | [UpdateModelRouteRequest](#updatemodelrouterequest) | [EmptyResponse](common.md#emptyresponse) |

## Payload schemas

Schemas use TypeScript notation for JSON objects and unions. `?` means omission is allowed; `null` is allowed only where written. `Record<symbol, never>` denotes the empty JSON object `{}`. Numeric bounds and accepted input defaults are specified in the owning contract.

### CreateModelRouteRequest

```typescript
export type CreateModelRouteRequest = {
    public_id: string;
    is_enabled: boolean;
    target: ModelRouteTarget;
};
```

Related types: [ModelRouteTarget](#modelroutetarget).

### ModelRouteTarget

```typescript
export type ModelRouteTarget =
    | {
          type: "direct";
          model_id: number;
      }
    | {
          type: "pool";
          pool_id: number;
          upstream_id: string;
      };
```

### CreateModelRouteResponse

```typescript
export type CreateModelRouteResponse = {
    model_route_id: number;
};
```

### DeleteModelRouteRequest

```typescript
export type DeleteModelRouteRequest = {
    model_route_id: number;
};
```

### ListModelRoutesRequest

```typescript
export type ListModelRoutesRequest = Record<symbol, never>;
```

### ListModelRoutesResponse

```typescript
export type ListModelRoutesResponse = {
    model_routes: Array<ModelRouteResponse>;
};
```

Related types: [ModelRouteResponse](#modelrouteresponse).

### ModelRouteResponse

```typescript
export type ModelRouteResponse = {
    is_automatic: boolean;
    id: number;
    public_id: string;
    is_enabled: boolean;
    target: ModelRouteTarget;
};
```

Related types: [ModelRouteTarget](#modelroutetarget).

### UpdateModelRouteRequest

```typescript
export type UpdateModelRouteRequest = {
    model_route_id: number;
    public_id: string;
    is_enabled: boolean;
    target: ModelRouteTarget;
};
```

Related types: [ModelRouteTarget](#modelroutetarget).

