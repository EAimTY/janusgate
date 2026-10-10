# Inference API

All client families share public routes, key policy, and five upstream formats. The gateway translates supported semantics; it does not implement every vendor API feature. Clients provide full history and a public model ID on each request. [Routing](../design/routing.md) owns account selection and [compaction](../design/compaction.md) owns context state.

## Endpoints and authentication

| Method | Path | Body / framing |
| --- | --- | --- |
| POST | `/api/openai-v1/chat/completions` | Chat Completions; `stream` selects SSE |
| POST | `/api/openai-v1/responses` | Responses; `stream` selects SSE |
| POST | `/api/openai-v1/responses/compact` | Compact request; JSON only |
| GET | `/api/openai-v1/models` | No body; JSON list |
| POST | `/api/gemini-v1beta/models/{model}:generateContent` | GenerateContent; JSON |
| POST | `/api/gemini-v1beta/models/{model}:streamGenerateContent?alt=sse` | GenerateContent; SSE, only `alt=sse` supported |
| POST | `/api/gemini-v1beta/interactions` | Interactions; `stream` or `alt=sse` selects SSE |
| GET | `/api/gemini-v1beta/models` | No body; JSON list |
| POST | `/api/anthropic-v1/messages` | Messages; `stream` selects SSE |
| GET | `/api/anthropic-v1/models` | No body; JSON list |

OpenAI uses `Authorization: Bearer <serving-key>` with case-insensitive scheme and byte-exact key. Gemini uses only `x-goog-api-key`, not a query key. Anthropic uses `x-api-key` and requires `anthropic-version` to be present without validating its value. Enabled keys and route policy apply to inference, compact, replay, and model listing. Requests are JSON with a 64 MiB body cap.

Unknown request members and known unsupported semantic controls produce 400s. The exceptions below accept presentation/replay members without changing execution. Definitions decide upstream framing; a streaming-only upstream can serve a non-streaming caller through aggregation.

## Supported requests

### OpenAI Chat Completions

Body fields: `model`, `messages`, optional function `tools`, `tool_choice`, `temperature`, `top_p`, `max_tokens` or `max_completion_tokens` (not both), `response_format` (text/JSON object/JSON schema), `reasoning_effort`, `stream`, and `stream_options.include_usage`.

Roles are system, developer, user, assistant, and tool. System/developer instructions concatenate in encounter order. System/developer/tool/assistant text can be a string or text-part array; tool-result media is rejected. Function arguments must be JSON objects encoded in their protocol string field. Legacy assistant `function_call` normalizes to a single tool call; modern `tool_calls` takes precedence.

Ignored members: message `name`; assistant `reasoning`, `reasoning_content`, `refusal`, `audio`, `annotations`; function-tool/JSON-schema `strict`; image `detail`. Gateway tool-call `thought_signature` is a replayable extension. Upstream reasoning prefers `reasoning` over `reasoning_content`; outward reasoning uses `reasoning_content`.

### OpenAI Responses

Body fields: `model`, `input` (string or ordered items), optional `instructions`, function `tools`, `tool_choice`, sampling, `max_output_tokens`, `reasoning.effort`, `reasoning.summary` (`auto` only), `stream`, `store` (absent/false only), `text.format` (text/JSON object/JSON schema), and compaction controls.

Input items are message, function_call, function_call_output, reasoning, compaction, and terminal compaction_trigger. System/developer content is text-only. Tool outputs accept string or text parts; image/file outputs are rejected. Reasoning with `encrypted_content` is native positioned state; without ciphertext it is accepted replay metadata. Stateful continuation and configuration-update items are unsupported.

Ignored envelope members: `parallel_tool_calls`, `include`, `prompt_cache_key`, `stream_options`, `service_tier`, `client_metadata`, `access_programs`; `text.verbosity`; tool/schema `strict`. Replay ignores message `id`, `phase`, `status`; call/result `id`, `status`; output-text `annotations`, `logprobs`; image `detail`. Function-call `thought_signature` round-trips Gemini signatures.

Standalone compact accepts only `model`, `input`, optional `instructions`, and ignored `prompt_cache_key`, `prompt_cache_options`, `prompt_cache_retention`, `service_tier`; `previous_response_id` must be absent/null. It has no stream/tools/sampling fields. See the [compact contract](../design/compaction.md#request-contracts).

### Gemini GenerateContent

Model and stream mode come from the URL action; the body does not carry model. Requests normalize system instructions, user/model contents with text/images/function calls/results, function tools, tool choice, sampling, output-token limit, structured text response, and `generationConfig.thinkingConfig.thinkingLevel` into shared semantics. URL and inline images are supported. Function response objects become JSON text internally and render as objects or text according to the target; any `functionResponse.parts` media is rejected. `scheduling` and `willContinue` are ignored.

Numeric thinking budgets, `topK`, stops, built-in tools, speech/audio/video generation, and other unknown controls are rejected. Prompt safety blocks become content-filter completion, preserving native prompt-block feedback where available.

### Gemini Interactions

Requests use `model`, `input`, optional `system_instruction`, function tools, `generation_config` (output-token limit, thinking level, tool choice), `response_format`, and `stream`. Input may be string, one content block, content blocks, or ordered replay steps. Text/images, model output/thought, function_call, and function_result map to shared messages. Tool arguments are JSON objects, not encoded strings.

Only text output is supported; JSON/schema use the text member with `application/json`. Sampling has no Interactions wire slot and is dropped when translating to this upstream. Tool choice is auto/any/none or one allowed function; validated/multiple allowed tools are rejected. Function-result objects flatten into text; media results are rejected.

Ignored members: `store`, `service_tier`, `labels`, `generation_config.thinking_summaries`, text annotations, and image resolution, except annotations inside opaque thought replay remain preserved. Upstream always renders `store: false` without previous interaction ID.

Rejected controls: `previous_interaction_id`, `background`, `cached_content`, `webhook_config`, response modalities, deprecated response MIME type, safety settings, agents/environments, and generation seed/stops/speech/image/video/transcription configuration. Audio/image/video response formats and a schema without JSON MIME type are rejected.

### Anthropic Messages

Requests normalize `model`, `messages`, optional system text, function tools, tool choice, sampling, `max_tokens`, `output_config.effort`, JSON-schema `output_config.format`, `stream`, and compaction controls. Text, base64/URL image, thinking/redacted-thinking replay, tool_use, and text tool_result blocks are supported. `is_error` survives only on formats carrying it. Document blocks, media tool results, built-in tools, top_k, stops, numeric thinking budgets, diagnostics, and inference geography are rejected.

Ignored controls: `metadata`, `service_tier`, `container`, `disable_parallel_tool_use`, cache_control, citations, and tool strict. A tool `type`, when supplied, must be custom. Adaptive/enabled thinking is accepted metadata; enabled/adaptive display is ignored. Disabled thinking maps to none. A format-only output_config is valid; null effort/format means absent.

Upstream rendering produces alternating turns beginning with user and merges consecutive same-role messages without crossing state boundaries. Missing upstream output cap synthesizes 16384. JSON-schema output preserves schema; free-form JSON object has no Messages slot and fails. Foreign or unsigned thinking is not fabricated into signed blocks.

## Translation and replay

Shared semantics include text, images, function calls/results, instructions, sampling, tool choice, structured text output, reasoning, usage, and positioned state. Tools are function-only; the gateway never executes them. Stops, seeds, token thinking budgets, and unsupported media/control mechanisms are rejected rather than approximated.

Reasoning effort is the closed vocabulary `none | minimal | low | medium | high | xhigh | max`. Unknown values are 400. Known values render verbatim to the target's level field, with Anthropic none rendered as disabled thinking. Omission uses the upstream default; levels are not mapped or checked against informational capability sets. Unsupported known values receive the upstream's rejection.

Same-family reasoning preserves replayable producer payloads. The two Gemini formats bridge their signature channels. Other cross-family opaque reasoning is omitted upstream; readable reasoning reaches compatible client text channels. Signed native state requires its own producer/prefix compatibility. [Compaction replay](../design/compaction.md#replacement-and-replay) must run before ordinary role merging.

## Responses, streaming, and usage

Non-streaming replies use each family’s native envelope: Chat Completions choices, Responses output, GenerateContent candidates, Interaction steps, and Messages content. Identifiers are renderer-owned except native standalone compact passthrough. Streamed/aggregated content, tool identity, state ordering, and terminal usage agree; unknown upstream fields are ignored, while wrong shapes in consumed fields fail as malformed payloads.

Chat Completions uses data chunks ending `[DONE]`; its optional usage chunk appears once immediately before it. Responses uses named events carrying matching `type` and monotonically increasing `sequence_number`, ending completed/incomplete or failed. Messages uses message/content-block lifecycles; complete tool arguments render in one delta. Interactions uses event names and `event_type` and recovers undelivered terminal steps without duplicate content. GenerateContent emits native candidate/usage chunks.

Length/content-filter finishes produce Responses incomplete with the corresponding reason. Ordinary Responses streams missing a terminal event may degrade to completion from observed state; compaction requires validated terminal success. No finish or usage is fabricated to authorize replacement of context.

Usage uses the last authoritative upstream report. Normalized output includes reasoning; native Gemini output/thought fields split it back out. Responses required breakdowns zero-fill unavailable details, while numerical accounting retains unknown. Messages preserves native reasoning breakdowns and reports compaction iterations separately. Totals sum the fields rendered in the target schema. [Accounting](../design/storage.md#request-accounting) distinguishes billed facts from response-required zeros.

## Model lists

Lists contain admitted structurally available public routes in name order, without quota filtering or pagination. OpenAI returns `{object: "list", data: [...]}` with public ID, model object, created 0, owner label, optional capacities/reasoning_levels/compaction. Gemini returns `{models: [...]}` without nextPageToken, with native input/output limits. Anthropic returns `{data, first_id, last_id, has_more: false}`, epoch created_at and public display names. Unknown capacities and support facts are omitted; [routing](../design/routing.md#public-model-listings) specifies pool composition.

## Errors

| Condition | HTTP |
| --- | --- |
| Invalid key / required auth header | 401 |
| Invalid/unsupported input or conflicting routing state | 400 |
| Unknown, disabled, or denied public model / unknown protocol path | 404 |
| Wrong method / body too large | 405 / 413 |
| Pool or required-account structurally unavailable | 503 |
| Pool or required-account quota exhaustion | 429, possibly Retry-After |
| Rejected upstream request | Mirrored upstream non-2xx status |
| Upstream transport/framing/payload failure | 502 |
| Internal gateway failure | 500, fixed safe message |

OpenAI renders `{error: {message, type, param, code}}`; Gemini `{error: {code, message, status}}`; Anthropic `{type: "error", error: {type, message}}`. Pool/bound failures retain their stable codes in OpenAI and their protocol category/context elsewhere. [Compaction](../design/compaction.md#failures) adds stable state/summary failure codes. Mirrored error text is bounded; raw bodies are not persisted. Mid-stream failures use the same classification in native error events after HTTP commitment. Unmatched paths outside protocol routers return empty 404.
