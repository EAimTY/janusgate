# Request usage

Generated from the Rust v0 route registry and serde-aware TypeScript bindings. Regenerate with `npm run docs:generate`; do not edit this page. Read the [management contract](../management-api.md) for transport, validation, defaults, and behavior.

## Routes

All paths below are complete. Success is HTTP 200 JSON.

| Method | Path | Request | Response |
| --- | --- | --- | --- |
| `POST` | `/management-api/v0/usage:list` | [ListUsageRequest](#listusagerequest) | [ListUsageResponse](#listusageresponse) |
| `POST` | `/management-api/v0/usage:query` | [QueryUsageRequest](#queryusagerequest) | [QueryUsageResponse](#queryusageresponse) |

## Payload schemas

Schemas use TypeScript notation for JSON objects and unions. `?` means omission is allowed; `null` is allowed only where written. `Record<symbol, never>` denotes the empty JSON object `{}`. Numeric bounds and accepted input defaults are specified in the owning contract.

### ListUsageRequest

```typescript
export type ListUsageRequest = {
    limit?: number;
    before?: number;
    filter?: UsageFilter;
};
```

Related types: [UsageFilter](#usagefilter).

### UsageFilter

```typescript
export type UsageFilter = {
    from?: number;
    to?: number;
    api_key_id?: Array<number>;
    provider_id?: Array<number>;
    model_id?: Array<number>;
    model_route_id?: Array<number>;
    endpoint_type?: Array<ApiLogEndpointType>;
    outcome?: Array<ApiLogOutcome>;
    is_stream?: boolean;
    http_status?: Array<number>;
};
```

Related types: [ApiLogEndpointType](#apilogendpointtype), [ApiLogOutcome](#apilogoutcome).

### ApiLogEndpointType

```typescript
export type ApiLogEndpointType =
    | "openai_chat_completions"
    | "openai_responses"
    | "openai_responses_compact"
    | "gemini_generate_content"
    | "gemini_interactions"
    | "anthropic_messages";
```

### ApiLogOutcome

```typescript
export type ApiLogOutcome = "completed" | "client_aborted" | "upstream_error" | "internal";
```

### ListUsageResponse

```typescript
export type ListUsageResponse = {
    api_logs: Array<ApiLogResponse>;
};
```

Related types: [ApiLogResponse](#apilogresponse).

### ApiLogResponse

```typescript
export type ApiLogResponse = {
    model_route_id: number;
    upstream_http_status: number | null;
    started_at: number;
    api_key_id: number;
    provider_id: number;
    model_id: number;
    endpoint_type: ApiLogEndpointType;
    is_stream: boolean;
    http_status: number | null;
    outcome: ApiLogOutcome;
    duration: number;
    input_tokens: number | null;
    output_tokens: number | null;
    cached_tokens: number | null;
    cache_write_tokens: number | null;
    reasoning_tokens: number | null;
};
```

Related types: [ApiLogEndpointType](#apilogendpointtype), [ApiLogOutcome](#apilogoutcome).

### QueryUsageRequest

```typescript
export type QueryUsageRequest = {
    group_by: Array<UsageGroupBy>;
    filter?: UsageFilter;
};
```

Related types: [UsageGroupBy](#usagegroupby), [UsageFilter](#usagefilter).

### UsageGroupBy

```typescript
export type UsageGroupBy =
    | "day"
    | "model"
    | "model_route"
    | "provider"
    | "api_key"
    | "endpoint_type"
    | "outcome"
    | "is_stream"
    | "http_status";
```

### QueryUsageResponse

```typescript
export type QueryUsageResponse = {
    buckets: Array<UsageBucket>;
};
```

Related types: [UsageBucket](#usagebucket).

### UsageBucket

```typescript
export type UsageBucket = {
    key: UsageBucketKey;
    requests: number;
    input_tokens: number;
    output_tokens: number;
    cached_tokens: number;
    cache_write_tokens: number;
    reasoning_tokens: number;
};
```

Related types: [UsageBucketKey](#usagebucketkey).

### UsageBucketKey

```typescript
export type UsageBucketKey = {
    model_route_id?: number;
    day?: string;
    model_id?: number;
    provider_id?: number;
    api_key_id?: number;
    endpoint_type?: ApiLogEndpointType;
    outcome?: ApiLogOutcome;
    is_stream?: boolean;
    http_status?: number | null;
};
```

Related types: [ApiLogEndpointType](#apilogendpointtype), [ApiLogOutcome](#apilogoutcome).

