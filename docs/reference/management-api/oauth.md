# OAuth flows

Generated from the Rust v0 route registry and serde-aware TypeScript bindings. Regenerate with `npm run docs:generate`; do not edit this page. Read the [management contract](../management-api.md) for transport, validation, defaults, and behavior.

## Routes

All paths below are complete. Success is HTTP 200 JSON.

| Method | Path | Request | Response |
| --- | --- | --- | --- |
| `POST` | `/management-api/v0/oauth:complete` | [CompleteProviderOauthRequest](#completeprovideroauthrequest) | [CompleteProviderOauthResponse](#completeprovideroauthresponse) |
| `POST` | `/management-api/v0/oauth:reauthorize` | [ReauthorizeProviderRequest](#reauthorizeproviderrequest) | [StartProviderOauthResponse](#startprovideroauthresponse) |
| `POST` | `/management-api/v0/oauth:start` | [StartProviderOauthRequest](#startprovideroauthrequest) | [StartProviderOauthResponse](#startprovideroauthresponse) |

## Payload schemas

Schemas use TypeScript notation for JSON objects and unions. `?` means omission is allowed; `null` is allowed only where written. `Record<symbol, never>` denotes the empty JSON object `{}`. Numeric bounds and accepted input defaults are specified in the owning contract.

### CompleteProviderOauthRequest

```typescript
export type CompleteProviderOauthRequest =
    | {
          grant: "authorization_code";
          state: string;
          code: string;
      }
    | {
          grant: "device";
          state: string;
      };
```

### CompleteProviderOauthResponse

```typescript
export type CompleteProviderOauthResponse =
    | {
          status: "succeeded";
          provider: ProviderResponse;
      }
    | {
          status: "pending";
      }
    | {
          status: "failed";
          failure_code: string;
          failure_message: string;
      };
```

Related types: [ProviderResponse](common.md#providerresponse).

### ReauthorizeProviderRequest

```typescript
export type ReauthorizeProviderRequest = {
    provider_id: number;
    provider_definition_name?: string;
    provider_definition_version?: number;
};
```

### StartProviderOauthResponse

```typescript
export type StartProviderOauthResponse =
    | {
          grant: "authorization_code";
          authorize_url: string;
          state: string;
          expires_at: number;
      }
    | {
          grant: "device";
          state: string;
          expires_at: number;
          user_code: string;
          verification_uri?: string;
          verification_uri_complete?: string;
      };
```

### StartProviderOauthRequest

```typescript
export type StartProviderOauthRequest = {
    model_prefix: string;
    provider_definition_name: string;
    provider_definition_version: number;
    name: string;
    base_url?: string;
};
```

