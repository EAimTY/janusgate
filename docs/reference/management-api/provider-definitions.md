# Provider definitions

Generated from the Rust v0 route registry and serde-aware TypeScript bindings. Regenerate with `npm run docs:generate`; do not edit this page. Read the [management contract](../management-api.md) for transport, validation, defaults, and behavior.

## Routes

All paths below are complete. Success is HTTP 200 JSON.

| Method | Path | Request | Response |
| --- | --- | --- | --- |
| `POST` | `/management-api/v0/provider-definitions:create` | [CreateProviderDefinitionRequest](#createproviderdefinitionrequest) | [DefinitionEnvelope](#definitionenvelope) |
| `POST` | `/management-api/v0/provider-definitions:list` | [ListProviderDefinitionsRequest](#listproviderdefinitionsrequest) | [ListProviderDefinitionsResponse](#listproviderdefinitionsresponse) |
| `POST` | `/management-api/v0/provider-definitions:update` | [UpdateProviderDefinitionRequest](#updateproviderdefinitionrequest) | [DefinitionEnvelope](#definitionenvelope) |

## Payload schemas

Schemas use TypeScript notation for JSON objects and unions. `?` means omission is allowed; `null` is allowed only where written. `Record<symbol, never>` denotes the empty JSON object `{}`. Numeric bounds and accepted input defaults are specified in the owning contract.

### CreateProviderDefinitionRequest

```typescript
export type CreateProviderDefinitionRequest = {
    name: string;
    format: UpstreamFormat;
    inference: Inference;
    credential: CredentialAcquisition;
    provisioning?: Array<ProvisioningStep> | null;
    discovery?: Discovery | null;
    quota?: QuotaDescriptor | null;
    compaction?: CompactionSection | null;
};
```

Related types: [UpstreamFormat](common.md#upstreamformat), [Inference](#inference), [CredentialAcquisition](#credentialacquisition), [ProvisioningStep](#provisioningstep), [Discovery](#discovery), [QuotaDescriptor](#quotadescriptor), [CompactionSection](#compactionsection).

### Inference

```typescript
export type Inference = {
    url: StringExpr;
    stream_url?: StringExpr | null;
    headers?: Array<ParamRow>;
    upstream_streaming: UpstreamStreaming;
    render_options?: Array<RenderOption>;
    envelope?: TemplateNode | null;
    quota_exhaustion?: QuotaExhaustion | null;
    response_unwrap?: string | null;
};
```

Related types: [StringExpr](#stringexpr), [ParamRow](#paramrow), [UpstreamStreaming](#upstreamstreaming), [RenderOption](#renderoption), [TemplateNode](#templatenode), [QuotaExhaustion](#quotaexhaustion).

### StringExpr

```typescript
export type StringExpr = Array<ValueExpr>;
```

Related types: [ValueExpr](#valueexpr).

### ValueExpr

```typescript
export type ValueExpr =
    | {
          type: "literal";
          value: string;
      }
    | {
          type: "secret";
      }
    | {
          type: "slot";
          name: string;
      }
    | {
          type: "client_id";
      }
    | {
          type: "client_secret";
      }
    | {
          type: "authorization_code";
      }
    | {
          type: "code_verifier";
      }
    | {
          type: "redirect_uri";
      }
    | {
          type: "oauth_state";
      }
    | {
          type: "device_code";
      }
    | {
          type: "refresh_token";
      }
    | {
          type: "request_uuid";
      }
    | {
          type: "model";
      }
    | {
          type: "base";
      }
    | {
          type: "coalesce";
          parts: Array<ValueExpr>;
      };
```

### ParamRow

```typescript
export type ParamRow = {
    name: string;
    value: StringExpr;
};
```

Related types: [StringExpr](#stringexpr).

### UpstreamStreaming

```typescript
export type UpstreamStreaming = "always" | "match_client";
```

### RenderOption

```typescript
export type RenderOption = "omit_max_output_tokens";
```

### TemplateNode

```typescript
export type TemplateNode =
    | {
          type: "json";
          value: JsonValue;
      }
    | {
          type: "string";
          expr: StringExpr;
      }
    | {
          type: "body";
      }
    | {
          type: "object";
          fields: { [key in string]: TemplateNode };
      }
    | {
          type: "array";
          items: Array<TemplateNode>;
      };
```

Related types: [JsonValue](#jsonvalue), [StringExpr](#stringexpr).

### JsonValue

```typescript
export type JsonValue =
    | number
    | string
    | boolean
    | Array<JsonValue>
    | { [key in string]: JsonValue }
    | null;
```

### QuotaExhaustion

```typescript
export type QuotaExhaustion = {
    rules: Array<QuotaExhaustionRule>;
};
```

Related types: [QuotaExhaustionRule](#quotaexhaustionrule).

### QuotaExhaustionRule

```typescript
export type QuotaExhaustionRule = {
    name: string;
    statuses: Array<RejectedStatus>;
    code: ExactQuotaCode;
    restriction_key: Array<RestrictionKeySegment>;
    applies_to: RejectionApplicability;
};
```

Related types: [RejectedStatus](#rejectedstatus), [ExactQuotaCode](#exactquotacode), [RestrictionKeySegment](#restrictionkeysegment), [RejectionApplicability](#rejectionapplicability).

### RejectedStatus

```typescript
export type RejectedStatus = number;
```

### ExactQuotaCode

```typescript
export type ExactQuotaCode = {
    at: ResponsePath;
    value: string;
};
```

Related types: [ResponsePath](#responsepath).

### ResponsePath

```typescript
export type ResponsePath = Array<string>;
```

### RestrictionKeySegment

```typescript
export type RestrictionKeySegment =
    | {
          type: "literal";
          value: string;
      }
    | {
          type: "requested_model";
      };
```

### RejectionApplicability

```typescript
export type RejectionApplicability =
    | {
          type: "all_models";
      }
    | {
          type: "requested_model";
      };
```

### CredentialAcquisition

```typescript
export type CredentialAcquisition =
    | {
          type: "api_key";
      }
    | ({
          type: "oauth";
      } & OAuthRecipe);
```

Related types: [OAuthRecipe](#oauthrecipe).

### OAuthRecipe

```typescript
export type OAuthRecipe = {
    client_id: string;
    client_secret?: string | null;
    access_token_lifetime_seconds?: number | null;
    initial_slots?: Array<SlotInitializer>;
    grant: Grant;
    exchange: Exchange;
    id_token_claims?: Array<IdTokenClaim>;
    refresh: TokenRefresh;
};
```

Related types: [SlotInitializer](#slotinitializer), [Grant](#grant), [Exchange](#exchange), [IdTokenClaim](#idtokenclaim), [TokenRefresh](#tokenrefresh).

### SlotInitializer

```typescript
export type SlotInitializer = {
    name: string;
    generator: SlotGenerator;
};
```

Related types: [SlotGenerator](#slotgenerator).

### SlotGenerator

```typescript
export type SlotGenerator = "uuid" | "uuid_hex";
```

### Grant

```typescript
export type Grant =
    | {
          type: "authorization_code";
          authorize_url: StringExpr;
          redirect_uri: string;
          pkce: Pkce;
          extra_params?: Array<ParamRow>;
      }
    | {
          type: "device";
          authorization: DeviceAuthorization;
          expires_in_fallback_seconds?: number | null;
      };
```

Related types: [StringExpr](#stringexpr), [Pkce](#pkce), [ParamRow](#paramrow), [DeviceAuthorization](#deviceauthorization).

### Pkce

```typescript
export type Pkce = "none" | "s256";
```

### DeviceAuthorization

```typescript
export type DeviceAuthorization = {
    request: ControlRequest;
    response: ResponseDescriptor;
};
```

Related types: [ControlRequest](#controlrequest), [ResponseDescriptor](#responsedescriptor).

### ControlRequest

```typescript
export type ControlRequest = {
    method: ControlMethod;
    url: StringExpr;
    headers?: Array<ParamRow>;
    query?: Array<ParamRow>;
    body?: ControlBody | null;
};
```

Related types: [ControlMethod](#controlmethod), [StringExpr](#stringexpr), [ParamRow](#paramrow), [ControlBody](#controlbody).

### ControlMethod

```typescript
export type ControlMethod = "GET" | "POST";
```

### ControlBody

```typescript
export type ControlBody =
    | {
          kind: "form";
          fields: Array<ParamRow>;
      }
    | {
          kind: "json";
          node: TemplateNode;
      };
```

Related types: [ParamRow](#paramrow), [TemplateNode](#templatenode).

### ResponseDescriptor

```typescript
export type ResponseDescriptor = {
    encoding: ResponseEncoding;
    fields: Array<ResponseField>;
};
```

Related types: [ResponseEncoding](#responseencoding), [ResponseField](#responsefield).

### ResponseEncoding

```typescript
export type ResponseEncoding = "json" | "form";
```

### ResponseField

```typescript
export type ResponseField = {
    path: ResponsePath;
    target: ResponseTarget;
    required: boolean;
};
```

Related types: [ResponsePath](#responsepath), [ResponseTarget](#responsetarget).

### ResponseTarget

```typescript
export type ResponseTarget =
    | {
          type: "access_token";
      }
    | {
          type: "refresh_token";
      }
    | {
          type: "expires_in_seconds";
      }
    | {
          type: "id_token";
      }
    | {
          type: "base_url";
      }
    | {
          type: "slot";
          name: string;
      }
    | {
          type: "device_code";
      }
    | {
          type: "user_code";
      }
    | {
          type: "verification_uri";
      }
    | {
          type: "verification_uri_complete";
      }
    | {
          type: "interval_seconds";
      }
    | {
          type: "expires_at_unix_seconds";
      };
```

### Exchange

```typescript
export type Exchange = {
    request: ControlRequest;
    response: ResponseDescriptor;
    terminal_codes: Array<string>;
};
```

Related types: [ControlRequest](#controlrequest), [ResponseDescriptor](#responsedescriptor).

### IdTokenClaim

```typescript
export type IdTokenClaim = {
    claim: ResponsePath;
    slot: string;
};
```

Related types: [ResponsePath](#responsepath).

### TokenRefresh

```typescript
export type TokenRefresh =
    | {
          type: "grant";
          request: ControlRequest;
          response?: ResponseDescriptor | null;
          terminal_codes: Array<string>;
      }
    | {
          type: "reprovision";
      }
    | {
          type: "none";
      };
```

Related types: [ControlRequest](#controlrequest), [ResponseDescriptor](#responsedescriptor).

### ProvisioningStep

```typescript
export type ProvisioningStep = {
    request: ControlRequest;
    response_encoding: ResponseEncoding;
    extract?: Array<ProvisionExtraction>;
    skip_if?: string | null;
    poll?: Poll | null;
};
```

Related types: [ControlRequest](#controlrequest), [ResponseEncoding](#responseencoding), [ProvisionExtraction](#provisionextraction), [Poll](#poll).

### ProvisionExtraction

```typescript
export type ProvisionExtraction = {
    path: ResponsePath;
    target: ResponseTarget;
    shape: ExtractionShape;
    required: boolean;
};
```

Related types: [ResponsePath](#responsepath), [ResponseTarget](#responsetarget), [ExtractionShape](#extractionshape).

### ExtractionShape

```typescript
export type ExtractionShape =
    | {
          type: "string";
      }
    | {
          type: "integer";
      }
    | {
          type: "string_or_field";
          field: string;
      };
```

### Poll

```typescript
export type Poll = {
    request: ControlRequest;
    response_encoding: ResponseEncoding;
    done_path: ResponsePath;
    error_path: ResponsePath;
    result_extract?: Array<ProvisionExtraction>;
    interval_ms: number;
    budget_ms: number;
};
```

Related types: [ControlRequest](#controlrequest), [ResponseEncoding](#responseencoding), [ResponsePath](#responsepath), [ProvisionExtraction](#provisionextraction).

### Discovery

```typescript
export type Discovery = {
    request: ControlRequest;
    extract: DiscoveryExtract;
    pagination?: Pagination | null;
};
```

Related types: [ControlRequest](#controlrequest), [DiscoveryExtract](#discoveryextract), [Pagination](#pagination).

### DiscoveryExtract

```typescript
export type DiscoveryExtract =
    | {
          mode: "array";
          array_at: ResponsePath;
          id_field: ResponsePath;
          strip_prefix?: string | null;
          capabilities?: CapabilityExtraction | null;
      }
    | {
          mode: "map_keys";
          at: ResponsePath;
      };
```

Related types: [ResponsePath](#responsepath), [CapabilityExtraction](#capabilityextraction).

### CapabilityExtraction

```typescript
export type CapabilityExtraction = {
    thinking_levels?: ThinkingLevelsExtraction | null;
    max_input_tokens?: ResponsePath | null;
    max_output_tokens?: ResponsePath | null;
    compaction?: CompactionCapabilityExtraction | null;
};
```

Related types: [ThinkingLevelsExtraction](#thinkinglevelsextraction), [ResponsePath](#responsepath), [CompactionCapabilityExtraction](#compactioncapabilityextraction).

### ThinkingLevelsExtraction

```typescript
export type ThinkingLevelsExtraction =
    | {
          shape: "member_array";
          array_at: ResponsePath;
          level_field: ResponsePath;
      }
    | {
          shape: "flagged_keys";
          object_at: ResponsePath;
          flag_field: ResponsePath;
      };
```

Related types: [ResponsePath](#responsepath).

### CompactionCapabilityExtraction

```typescript
export type CompactionCapabilityExtraction = {
    messages_on_demand?: FlaggedMemberExtraction | null;
    messages_threshold?: FlaggedMemberExtraction | null;
};
```

Related types: [FlaggedMemberExtraction](#flaggedmemberextraction).

### FlaggedMemberExtraction

```typescript
export type FlaggedMemberExtraction = {
    object_at: ResponsePath;
    flag_field: ResponsePath;
};
```

Related types: [ResponsePath](#responsepath).

### Pagination

```typescript
export type Pagination =
    | {
          type: "none";
      }
    | {
          type: "token";
          request_param: string;
          response_path: ResponsePath;
          max_pages: number;
      }
    | {
          type: "cursor";
          has_more_path: ResponsePath;
          last_id_path: ResponsePath;
          request_param: string;
          max_pages: number;
      };
```

Related types: [ResponsePath](#responsepath).

### QuotaDescriptor

```typescript
export type QuotaDescriptor = {
    request: ControlRequest;
    sources: Array<QuotaSource>;
};
```

Related types: [ControlRequest](#controlrequest), [QuotaSource](#quotasource).

### QuotaSource

```typescript
export type QuotaSource =
    | {
          type: "object";
          key: string;
          at: ResponsePath;
          is_required: boolean;
          scope?: QuotaTextSource;
          entries: Array<QuotaEntryExtraction>;
      }
    | {
          type: "array";
          key: string;
          at: ResponsePath;
          is_required: boolean;
          identity: Array<QuotaTextSource>;
          scope?: QuotaTextSource;
          entries: Array<QuotaEntryExtraction>;
      }
    | {
          type: "map";
          key: string;
          at: ResponsePath;
          is_required: boolean;
          scope?: QuotaTextSource;
          entries: Array<QuotaEntryExtraction>;
      };
```

Related types: [ResponsePath](#responsepath), [QuotaTextSource](#quotatextsource), [QuotaEntryExtraction](#quotaentryextraction).

### QuotaTextSource

```typescript
export type QuotaTextSource =
    | {
          type: "literal";
          value: string;
      }
    | {
          type: "field";
          field: QuotaField;
          encoding: QuotaTextEncoding;
      }
    | {
          type: "map_key";
      };
```

Related types: [QuotaField](#quotafield), [QuotaTextEncoding](#quotatextencoding).

### QuotaField

```typescript
export type QuotaField = {
    base: QuotaFieldBase;
    path: ResponsePath;
    is_required: boolean;
};
```

Related types: [QuotaFieldBase](#quotafieldbase), [ResponsePath](#responsepath).

### QuotaFieldBase

```typescript
export type QuotaFieldBase = "root" | "item" | "entry";
```

### QuotaTextEncoding

```typescript
export type QuotaTextEncoding = "string" | "integer";
```

### QuotaEntryExtraction

```typescript
export type QuotaEntryExtraction = {
    key: string;
    label: string;
    at: ResponsePath;
    is_required: boolean;
    window: QuotaWindowSource;
    measurement: QuotaMeasurementSource;
    reset: QuotaResetSource;
    routing: QuotaRoutingSource;
    is_unlimited?: QuotaField;
};
```

Related types: [ResponsePath](#responsepath), [QuotaWindowSource](#quotawindowsource), [QuotaMeasurementSource](#quotameasurementsource), [QuotaResetSource](#quotaresetsource), [QuotaRoutingSource](#quotaroutingsource), [QuotaField](#quotafield).

### QuotaWindowSource

```typescript
export type QuotaWindowSource =
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
      }
    | {
          type: "duration_field";
          value: QuotaNumberField;
          unit: QuotaDurationUnit;
      };
```

Related types: [SafeDuration](common.md#safeduration), [QuotaCalendarUnit](common.md#quotacalendarunit), [QuotaNumberField](#quotanumberfield), [QuotaDurationUnit](#quotadurationunit).

### QuotaNumberField

```typescript
export type QuotaNumberField = {
    field: QuotaField;
    encoding: QuotaNumberEncoding;
};
```

Related types: [QuotaField](#quotafield), [QuotaNumberEncoding](#quotanumberencoding).

### QuotaNumberEncoding

```typescript
export type QuotaNumberEncoding = "number" | "numeric_string" | "number_or_numeric_string";
```

### QuotaDurationUnit

```typescript
export type QuotaDurationUnit = "seconds" | "minutes" | "hours" | "days" | "weeks";
```

### QuotaMeasurementSource

```typescript
export type QuotaMeasurementSource =
    | {
          type: "unknown";
      }
    | {
          type: "ratio";
          value: QuotaNumberField;
          scale: QuotaRatioScale;
          direction: QuotaDirection;
      }
    | {
          type: "counter";
          value: QuotaNumberField;
          limit: QuotaNumberField;
          direction: QuotaDirection;
          unit: string;
      }
    | {
          type: "remaining";
          value: QuotaNumberField;
          unit: string;
      };
```

Related types: [QuotaNumberField](#quotanumberfield), [QuotaRatioScale](#quotaratioscale), [QuotaDirection](#quotadirection).

### QuotaRatioScale

```typescript
export type QuotaRatioScale = "fraction" | "percent";
```

### QuotaDirection

```typescript
export type QuotaDirection = "used" | "remaining";
```

### QuotaResetSource

```typescript
export type QuotaResetSource =
    | {
          type: "unknown";
      }
    | {
          type: "field";
          field: QuotaField;
          encoding: QuotaResetEncoding;
      };
```

Related types: [QuotaField](#quotafield), [QuotaResetEncoding](#quotaresetencoding).

### QuotaResetEncoding

```typescript
export type QuotaResetEncoding = "unix_seconds" | "unix_milliseconds" | "rfc3339" | "date";
```

### QuotaRoutingSource

```typescript
export type QuotaRoutingSource =
    | {
          type: "monitoring_only";
      }
    | {
          type: "all_models";
      }
    | {
          type: "models";
          upstream_ids: Array<QuotaTextSource>;
      };
```

Related types: [QuotaTextSource](#quotatextsource).

### CompactionSection

```typescript
export type CompactionSection = {
    strategy: CompactionStrategy;
    summary_budget: number;
    retention_budget: number;
    can_export_native_summary: boolean;
    mechanisms: MechanismDeclarations;
    count_tokens?: CountTokensOperation | null;
};
```

Related types: [CompactionStrategy](#compactionstrategy), [MechanismDeclarations](#mechanismdeclarations), [CountTokensOperation](#counttokensoperation).

### CompactionStrategy

```typescript
export type CompactionStrategy = "native_preferred" | "gateway_summary";
```

### MechanismDeclarations

```typescript
export type MechanismDeclarations = {
    responses_standalone_compact?: StandaloneCompactDeclaration | null;
    responses_explicit_trigger?: RideAlongDeclaration | null;
    responses_automatic_compaction?: RideAlongDeclaration | null;
    messages_on_demand_compaction?: OverlayDeclaration | null;
    messages_threshold_compaction?: OverlayDeclaration | null;
};
```

Related types: [StandaloneCompactDeclaration](#standalonecompactdeclaration), [RideAlongDeclaration](#ridealongdeclaration), [OverlayDeclaration](#overlaydeclaration).

### StandaloneCompactDeclaration

```typescript
export type StandaloneCompactDeclaration = {
    default: MechanismSupport;
    operation?: StandaloneCompactOperation | null;
    unsupported?: UnsupportedClassifier | null;
};
```

Related types: [MechanismSupport](#mechanismsupport), [StandaloneCompactOperation](#standalonecompactoperation), [UnsupportedClassifier](#unsupportedclassifier).

### MechanismSupport

```typescript
export type MechanismSupport = "supported" | "unsupported" | "unknown";
```

### StandaloneCompactOperation

```typescript
export type StandaloneCompactOperation = {
    url: StringExpr;
    headers?: Array<ParamRow>;
    envelope?: TemplateNode | null;
    response_unwrap?: string | null;
    unsupported?: UnsupportedClassifier | null;
};
```

Related types: [StringExpr](#stringexpr), [ParamRow](#paramrow), [TemplateNode](#templatenode), [UnsupportedClassifier](#unsupportedclassifier).

### UnsupportedClassifier

```typescript
export type UnsupportedClassifier = {
    statuses?: Array<RejectedStatus>;
    codes?: Array<string>;
    parameters?: Array<string>;
};
```

Related types: [RejectedStatus](#rejectedstatus).

### RideAlongDeclaration

```typescript
export type RideAlongDeclaration = {
    default: MechanismSupport;
    unsupported?: UnsupportedClassifier | null;
};
```

Related types: [MechanismSupport](#mechanismsupport), [UnsupportedClassifier](#unsupportedclassifier).

### OverlayDeclaration

```typescript
export type OverlayDeclaration = {
    default: MechanismSupport;
    overlay_headers?: Array<ParamRow> | null;
    unsupported?: UnsupportedClassifier | null;
};
```

Related types: [MechanismSupport](#mechanismsupport), [ParamRow](#paramrow), [UnsupportedClassifier](#unsupportedclassifier).

### CountTokensOperation

```typescript
export type CountTokensOperation = {
    url: StringExpr;
    headers?: Array<ParamRow>;
    envelope?: TemplateNode | null;
    response_unwrap?: string | null;
};
```

Related types: [StringExpr](#stringexpr), [ParamRow](#paramrow), [TemplateNode](#templatenode).

### DefinitionEnvelope

```typescript
export type DefinitionEnvelope = {
    provider_definition: DefinitionResponse;
};
```

Related types: [DefinitionResponse](#definitionresponse).

### DefinitionResponse

```typescript
export type DefinitionResponse = {
    name: string;
    version: number;
    created_at: number;
    format: UpstreamFormat;
    inference: Inference;
    credential: CredentialAcquisition;
    provisioning?: Array<ProvisioningStep> | null;
    discovery?: Discovery | null;
    quota?: QuotaDescriptor | null;
    compaction?: CompactionSection | null;
};
```

Related types: [UpstreamFormat](common.md#upstreamformat), [Inference](#inference), [CredentialAcquisition](#credentialacquisition), [ProvisioningStep](#provisioningstep), [Discovery](#discovery), [QuotaDescriptor](#quotadescriptor), [CompactionSection](#compactionsection).

### ListProviderDefinitionsRequest

```typescript
export type ListProviderDefinitionsRequest = Record<symbol, never>;
```

### ListProviderDefinitionsResponse

```typescript
export type ListProviderDefinitionsResponse = {
    provider_definitions: Array<DefinitionResponse>;
};
```

Related types: [DefinitionResponse](#definitionresponse).

### UpdateProviderDefinitionRequest

```typescript
export type UpdateProviderDefinitionRequest = {
    name: string;
    base_version: number;
    format: UpstreamFormat;
    inference: Inference;
    credential: CredentialAcquisition;
    provisioning?: Array<ProvisioningStep> | null;
    discovery?: Discovery | null;
    quota?: QuotaDescriptor | null;
    compaction?: CompactionSection | null;
};
```

Related types: [UpstreamFormat](common.md#upstreamformat), [Inference](#inference), [CredentialAcquisition](#credentialacquisition), [ProvisioningStep](#provisioningstep), [Discovery](#discovery), [QuotaDescriptor](#quotadescriptor), [CompactionSection](#compactionsection).

