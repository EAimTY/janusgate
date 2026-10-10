# Shared payload types

Generated from the Rust v0 route registry and serde-aware TypeScript bindings. Regenerate with `npm run docs:generate`; do not edit this page. Read the [management contract](../management-api.md) for transport, validation, defaults, and behavior.

## Payload schemas

Schemas use TypeScript notation for JSON objects and unions. `?` means omission is allowed; `null` is allowed only where written. `Record<symbol, never>` denotes the empty JSON object `{}`. Numeric bounds and accepted input defaults are specified in the owning contract.

### EmptyResponse

```typescript
export type EmptyResponse = Record<symbol, never>;
```

### ProviderResponse

```typescript
export type ProviderResponse =
    | {
          type: "oauth";
          expires_at: number | null;
          id: number;
          name: string;
          model_prefix: string;
          is_enabled: boolean;
          provider_definition_name: string;
          provider_definition_version: number;
          format: UpstreamFormat;
          base_url: string | null;
      }
    | {
          type: "api_key";
          id: number;
          name: string;
          model_prefix: string;
          is_enabled: boolean;
          provider_definition_name: string;
          provider_definition_version: number;
          format: UpstreamFormat;
          base_url: string | null;
      };
```

Related types: [UpstreamFormat](#upstreamformat).

### UpstreamFormat

```typescript
export type UpstreamFormat =
    | "openai_response"
    | "openai_chat_completion"
    | "gemini_generate_content"
    | "gemini_interaction"
    | "anthropic_messages";
```

### SafeDuration

```typescript
export type SafeDuration = number;
```

### QuotaCalendarUnit

```typescript
export type QuotaCalendarUnit = "month";
```

### ErrorCode

```typescript
export enum ErrorCode {
    InvalidRequest = 1,
    Unauthorized = 2,
    NotFound = 3,
    Conflict = 4,
    UpstreamError = 5,
    Internal = 6,
    PayloadTooLarge = 7,
    MethodNotAllowed = 8,
    UnsupportedMediaType = 9,
}
```

### ErrorResponse

```typescript
export type ErrorResponse = {
    errno: ErrorCode;
    message: string;
};
```

Related types: [ErrorCode](#errorcode).

