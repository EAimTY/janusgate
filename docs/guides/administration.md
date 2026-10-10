# Set up an instance

Build and run the gateway and console using the [quick start](../../README.md#get-started). Connection failures usually involve the wrong management token, a missing `management_api` configuration, CORS, or an HTTPS console attempting to contact an HTTP gateway. Configure the exact console origin in `management_api.cors.allowed_origin`; enabling or changing CORS requires a restart. The console sends the token verbatim, without Bearer.

## Connect an upstream account

In the console, create a provider using a suitable preset or an authored provider definition. A definition supplies protocol/request recipes; a provider supplies one account's credentials, optional base URL, and public model prefix. OAuth accounts complete the appropriate authorization or device flow. Definitions are immutable versions, so accounts bind an exact version.

Presets are editable recipe starting points. Confirm the upstream's current endpoints, scopes, client identity, discovery, and quota behavior before using one; the gateway does not infer compatibility from a brand name. [Provider definitions](../design/providers.md) explain these contracts.

For automation, every management call is POST JSON at `/management-api/v0/<action>`, using the raw management token. The [resource reference](../reference/management-api/README.md) supplies every request and response schema. This minimal static-key recipe supports OpenAI Chat Completions without model discovery:

```json
{
  "name": "chat-completions",
  "format": "openai_chat_completion",
  "inference": {
    "url": [{"type": "base"}, {"type": "literal", "value": "/chat/completions"}],
    "headers": [{"name": "Authorization", "value": [{"type": "literal", "value": "Bearer "}, {"type": "secret"}]}],
    "upstream_streaming": "match_client"
  },
  "credential": {"type": "api_key"}
}
```

Send this to `provider-definitions:create`, then use its returned version to create a provider with `providers:create`. Use the generated schemas to supply the full provider state, including `name`, `model_prefix`, `base_url`, `is_enabled`, and `api_key`; credentials belong in that request, not the definition. For a typical compatible upstream the base URL ends in `/v1`, with no trailing slash. Definition URL expressions concatenate literally.

## Make models callable

Discover models when the definition supports discovery; otherwise create physical models manually using the exact upstream IDs. Discovery retains manual rows and administrator overrides. A failed or empty discovery does not erase inventory. Vanished discovered rows retain identity and can be claimed manually.

A standalone provider automatically exposes `<provider-prefix><upstream-id>`. No alias creation is needed. Prefixes may be empty, but public names are globally unique. Adding a provider to any pool suppresses its standalone names; each pool exposes `<pool-prefix><upstream-id>` for the union of member inventory. Membership has unique nonnegative priorities, and eligibility still requires the exact physical model to exist and be enabled.

Create explicit direct aliases or pool routes only when you need additional names. Automatic entries are administered through their owners' prefixes and inventory. Prefix changes rename these entries while preserving route identities; a collision rejects the whole change. See [routing](../design/routing.md).

## Issue a serving key

Create an enabled serving key and grant access. Console drafts begin with a deny default. Choose provider or pool defaults for broad access, and public-route overrides for exceptions. A direct route inherits its provider rule; a pooled route inherits its pool rule, independently of member-provider rules. Explicit overrides survive renames because they attach to stable IDs.

Copy the key and one of the [client base URLs](../../README.md#4-point-a-client-at-janusgate). Model discovery returns only eligible names allowed by that key. Use the public name in inference requests; upstream IDs are passed through after routing. A management token is not a serving key.

## Operate and recover

Disabled accounts/models remain administrable. Reauthorize an expired OAuth account to retain its identity, models, routes, and access rules. Ordinary renewal happens on demand and preserves quota restrictions; explicit credential replacement or reauthorization clears them. Deleting and recreating an account creates new identities.

Quota views perform explicit upstream reads; availability views read stored restrictions. Resetting availability clears the gateway's evidence and grants a refresh grace interval; it cannot increase upstream quota. Pools preflight quota and fall through only on a proven first-dispatch quota rejection. Authentication failures, timeouts, overload, and bare 429s do not trigger account switching. Bound conversations must stay on their account. [Routing and quota](../design/routing.md) specify the failure behavior.

Usage records numerical upstream attempts, not prompts or responses. One client request may produce multiple rows, including compaction calls. Unknown usage is null. These records are best-effort operational accounting; [storage](../design/storage.md) describes retention and pagination limitations.

Protect the management token, serving keys, database, backups, and configuration as secrets. Use HTTPS for remote access. Gateway CORS is not authorization. Compaction carrier keys are optional until a request needs a sealed replay carrier; configure them before using gateway summaries, and retain old keys through rotation. See [configuration](../reference/configuration.md).
