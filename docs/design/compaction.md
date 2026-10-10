# Remote compaction

Compaction is client-directed and stateless. Only an explicit compact operation, trigger, or threshold control starts work. Replaying an artifact starts no new summary. Results live with the client; the gateway stores no conversation or artifact content.

## Controls and support

| Mechanism | Caller control | Result |
| --- | --- | --- |
| `responses_standalone_compact` | `POST /api/openai-v1/responses/compact` | Non-streaming canonical next input window |
| `responses_explicit_trigger` | Last Responses input item `compaction_trigger` | Exactly one completed compaction item, no answer |
| `responses_automatic_compaction` | Responses `context_management` compaction entries | Checkpoint(s) and continued answer |
| `messages_on_demand_compaction` | Messages `compaction: {type: "summarize", instructions?}` | Complete compaction block and `stop_reason: "compaction"` |
| `messages_threshold_compaction` | Messages `context_management.edits` with `compact_20260112` | Compaction then pause or continued answer |

A mechanism resolves administrator override → discovery fact → definition default → unknown. An explicit unknown override suppresses lower facts. Native execution requires supported state, configured operation data, and compatible replay semantics; unknown selects gateway summarization without probing.

Strategy `native_preferred` tries the caller's mechanism, compatible adapters, then gateway summary. Responses standalone uses native standalone or summary. Responses trigger can adapt a standalone window or Messages on-demand block. Messages on-demand can adapt a standalone Responses window. Threshold emulation can adapt supported explicit operations over older history. `gateway_summary` skips native selection for new work, while existing native state still obeys compatibility.

A typed positive unsupported classifier can allow one same-account summary fallback before any artifact commitment or visible content. Authentication, network errors, overload, arbitrary rejections, and positive quota evidence do not become unsupported-mechanism fallback. Successful headers or an earlier counter/summary/native stage commit the selected account; [pool fallback](routing.md#ordered-pool-selection) cannot move it afterward.

## Request contracts

Responses standalone accepts `model`, `input`, optional `instructions`, and the ignored caching/tier controls listed in the [inference reference](../reference/inference.md). It accepts no sampling, tools, reasoning, format, or stream controls. A non-null `previous_response_id` and any trigger item are rejected. The result is `{id, object: "response.compaction", created_at, output, usage}`; native passthrough preserves the complete ordered output window and optional `dropped_message_count`.

A Responses trigger must be unique and last, and cannot coexist with `context_management`; configuration-update items are rejected. Trigger success requires exactly one completed state item under a successful terminal lifecycle. Missing-terminal degradation never authorizes history replacement.

Messages on-demand cannot coexist with context management, stop sequences, output format, or forced tool choice; it requires history. Instructions have a 16384-character cap, with null/blank treated as absent. Native refusal, truncation, tool-use, exhausted-window, and null/no-summary outcomes preserve their protocol meanings and original history; HTTP 200 alone proves no usable replacement.

Messages threshold triggers are `input_tokens`, default 150000 and minimum 50000; invalid values are rejected, never clamped. Native historical compaction state cannot coexist with a threshold edit. On-demand/leading native blocks require inbound beta `compact-2026-09-04`; threshold/historical blocks require `compact-2026-01-12`. Inbound requirements are independent of definition-owned outbound headers. At most one inbound Messages compaction block is accepted.

## Replacement and replay

Every recognizable gateway carrier is authenticated before route selection, including carriers later superseded. Damaged markers and unknown keys are client-state errors, never reclassified as native data. A valid carrier still requires an admitted route. The latest usable boundary wins before role merging; null/no-op blocks establish no boundary.

Gateway summaries reconstruct preserved instructions, ordinary user-role summary context, exact retained interaction units/state, then fresh suffix. The summary is context, never new instruction authority. Proven source identities or semantic overlap remove covered instructions without dropping newly injected instructions. State at the same message offset retains encounter order, so covered state and fresh state stay distinct.

Wrapped native state binds its originating provider instance, upstream model, format, and any exact system/tool-prefix requirements, including omission/removal. Raw native replay remains the caller's compatibility responsibility; wrapping does not make state portable. Active authenticated provenance constrains pooled selection to the producer even after affinity loss; incompatible provenance or conflicting live requirements fail before dispatch.

`can_export_native_summary` permits readable native summary downgrade only when no ciphertext, signed reasoning, tool changes, or retained opaque state would be lost. Otherwise inaccessible native state fails explicitly. Gateway carrier markers never reach an upstream.

## Gateway summary and retention

Summary inference runs once on the same selected provider/model with normal credential handling. Supplied instructions replace the default summarization prompt. Tools are disabled; output schemas and stop controls do not propagate. Default output/retention budgets are 4096/8192 tokens; compact-only caller output caps may reduce summary output, while later answers retain their original budget.

History splits into indivisible interaction units. Parallel calls and their results stay together; orphan results are invalid. Compact-and-continue retains the pending user turn. Retention takes recent whole units backward under a soft budget; mandatory state and image suffixes are never truncated. No older summarizable prefix yields a budget error. Opaque state in the summarized region cannot be converted into text; retained state remains exact.

Only a complete, nonempty, tool-free summary ending normally is accepted. Refusal, truncation, empty output, or tool requests fail without retry or provider switching. Re-compaction restores the prior window and treats its summary as history, emitting a new checkpoint.

## Threshold measurement

Gateway emulation evaluates once at the request boundary. Below threshold, controls are stripped and ordinary generation proceeds. At/above threshold, one compaction installs its window, then pauses or generates the real answer. There is no re-evaluation loop.

A configured Messages counter measures the restored context; failure falls back to the text estimate `ceil(UTF-8 bytes / 3)` over deterministic model-visible text/tool context. The estimate is not a tokenizer or bound. Images and opaque state require a successful counter or fail measurement; transport-byte counting is forbidden. Measurements never become billed usage.

Emulated Responses with no threshold uses `floor(0.8 × effective max_input_tokens)`; absent capacity fails explicitly. Native automatic operations retain upstream defaults/timing. Native Messages threshold may compact repeatedly within one request, preserving each complete block, position, pause, and iteration usage.

## Carriers and delivery

Gateway carriers contain a versioned envelope with artifact identity, replacement semantics, preserved summary/retained state or a complete native window, and optional visible-content binding. They exclude credentials and a copied full uncompressed conversation. XChaCha20-Poly1305 authenticates the envelope under configured sealing keys:

```text
janusgate.compaction.v1.<key-id>.<base64url(nonce || ciphertext)>
```

Responses carries it in a compaction item's `encrypted_content`. Messages carries it in a compaction block's `signature`, binding the visible summary or fixed encrypted-state description. Native signatures remain unchanged. The first configured key seals; all retained keys can unseal. Prepend a new active key while retaining old keys for rotation; removing a key invalidates its artifacts. Missing sealing configuration prevents gateway carrier creation/consumption, while native passthrough remains available.

One outer lifecycle contains all internal calls. Compact-only results validate before publishing success, summary text never leaks as answer deltas, and complete checkpoints precede answers in JSON and SSE. A completed checkpoint remains valid after a later generation failure; an incomplete artifact never permits history replacement. Distinct native artifacts deduplicate by identity, not text similarity.

## Usage

Accounting sums reported native/summary/generation iteration usage for the physical-account attempt, including reported stages before failure. Missing usage stays unknown; counters and estimates create no billed token facts. Messages `usage.iterations` reports compaction iterations separately; its top-level usage reports non-compaction message work, zero for compact-only. Synthesized Responses reports participating billed totals in its required usage shape; native standalone preserves native usage. Replay costs only its actual inference, with no compaction call.

## Failures

| Stable code | HTTP | Meaning |
| --- | --- | --- |
| `compaction_unusable_history` | 400 | Invalid interaction structure |
| `compaction_inaccessible_state` | 400 | Incompatible or unusable native state |
| `compaction_invalid_gateway_state` | 400 | Failed carrier authentication/content binding |
| `compaction_unsupported_measurement` | 400 | Threshold context cannot be measured |
| `compaction_unknown_capacity` | 400 | No capacity for an implicit gateway threshold |
| `compaction_budget_exceeded` | 400 | Mandatory state leaves no summarizable prefix |
| `compaction_summary_unusable` | 502 | Summary refused, truncated, empty, or tool-producing |
| `compaction_configuration_missing` | 500 | Required sealing configuration absent |

Other upstream failures retain the inference error policy. Cancellation before artifact completion publishes no successful compaction result.
