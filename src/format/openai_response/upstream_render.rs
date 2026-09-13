//! Upstream-request rendering for the Responses codec: renders an IR request
//! into the `openai_response` upstream payload.

use crate::{
    format,
    ir::{IrPart, IrRequest, IrResponseFormat, IrRole, IrToolChoice},
};
use serde_json::Value;

/// Renders the Codex-flavored upstream request URL and payload: streaming is
/// forced on (README §5.1 invariant 2) and `max_output_tokens` is omitted —
/// the ChatGPT Codex backend rejects it as an unsupported parameter, and the
/// model's own output bound applies instead of the client's cap.
///
/// `url` is the fixed Codex inference endpoint; the provider layer owns
/// endpoint knowledge, so it is threaded in rather than owned by the codec.
///
/// # Panics
///
/// Panics if the rendered Responses body is not a JSON object, which
/// [`render_request_body`]'s construction makes impossible.
pub(crate) fn render_codex_request(ir: &IrRequest, url: &str) -> (String, Value) {
    let mut body = render_request_body(ir, true);

    // render_request_body always yields a JSON object, so the removal lands.
    #[expect(
        clippy::expect_used,
        reason = "render_request_body always renders a JSON object body"
    )]
    let object = body
        .as_object_mut()
        .expect("the rendered Responses body should be a JSON object");

    object.remove("max_output_tokens");

    (url.to_string(), body)
}

/// Renders the upstream request URL and payload for an OpenAI-compatible
/// Responses upstream: `POST <base_url>/responses` (README §5.3).
pub(crate) fn render_request(
    ir: &IrRequest,
    base_url: &str,
    is_streaming: bool,
) -> (String, Value) {
    let url = format!("{}/responses", base_url.trim_end_matches('/'));

    (url, render_request_body(ir, is_streaming))
}

/// Renders an IR request into the upstream Responses payload shared by the
/// Codex and compatible entry points.
fn render_request_body(ir: &IrRequest, is_streaming: bool) -> Value {
    let mut input: Vec<Value> = Vec::new();

    for message in &ir.messages {
        match message.role {
            IrRole::User => {
                let content: Vec<Value> = message
                    .parts
                    .iter()
                    .filter_map(|part| match part {
                        IrPart::Text { text } => {
                            Some(serde_json::json!({"type": "input_text", "text": text}))
                        }

                        IrPart::Image { image } => Some(serde_json::json!({
                            "type": "input_image",
                            "image_url": image.to_url(),
                        })),

                        // The IR type system routes only text and images into
                        // user content.
                        _ => None,
                    })
                    .collect();

                input.push(serde_json::json!({
                    "type": "message",
                    "role": "user",
                    "content": content,
                }));
            }

            IrRole::Assistant => {
                for part in &message.parts {
                    match part {
                        IrPart::Text { text } => {
                            input.push(serde_json::json!({
                                "type": "message",
                                "role": "assistant",
                                "content": [{"type": "output_text", "text": text}],
                            }));
                        }

                        IrPart::ToolCall {
                            call_id,
                            name,
                            arguments,
                            thought_signature,
                        } => {
                            let mut item = serde_json::json!({
                                "type": "function_call",
                                "call_id": call_id,
                                "name": name,
                                "arguments": arguments,
                            });

                            format::insert_thought_signature(&mut item, thought_signature);

                            input.push(item);
                        }

                        _ => {}
                    }
                }
            }

            IrRole::Tool => {
                for part in &message.parts {
                    if let IrPart::ToolOutput { call_id, content } = part {
                        input.push(serde_json::json!({
                            "type": "function_call_output",
                            "call_id": call_id,
                            "output": content,
                        }));
                    }
                }
            }
        }
    }

    let mut body = serde_json::json!({
        "model": ir.model,
        "input": input,
        "stream": is_streaming,
        "store": false,
    });

    if let Some(instructions) = &ir.instructions {
        format::json_insert(
            &mut body,
            "instructions",
            Value::String(instructions.clone()),
        );
    }

    if !ir.tools.is_empty() {
        let tools: Vec<Value> = ir
            .tools
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "type": "function",
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                })
            })
            .collect();

        format::json_insert(&mut body, "tools", Value::Array(tools));
    }

    if let Some(tool_choice) = &ir.tool_choice {
        let choice = match tool_choice {
            IrToolChoice::Auto => Value::String("auto".to_string()),
            IrToolChoice::None => Value::String("none".to_string()),
            IrToolChoice::Required => Value::String("required".to_string()),
            IrToolChoice::Function { name } => {
                serde_json::json!({"type": "function", "name": name})
            }
        };

        format::json_insert(&mut body, "tool_choice", choice);
    }

    if let Some(temperature) = ir.temperature {
        format::json_insert(&mut body, "temperature", Value::from(temperature));
    }

    if let Some(top_p) = ir.top_p {
        format::json_insert(&mut body, "top_p", Value::from(top_p));
    }

    if let Some(max_output_tokens) = ir.max_output_tokens {
        format::json_insert(
            &mut body,
            "max_output_tokens",
            Value::from(max_output_tokens),
        );
    }

    if let Some(response_format) = &ir.response_format {
        let format = match response_format {
            IrResponseFormat::Text => serde_json::json!({"type": "text"}),
            IrResponseFormat::Json => serde_json::json!({"type": "json_object"}),
            IrResponseFormat::JsonSchema { name, schema } => {
                serde_json::json!({"type": "json_schema", "name": name, "schema": schema})
            }
        };

        format::json_insert(&mut body, "text", serde_json::json!({"format": format}));
    }

    if let Some(effort) = ir.reasoning_effort {
        format::json_insert(
            &mut body,
            "reasoning",
            serde_json::json!({"effort": effort.to_openai()}),
        );
    }

    body
}
