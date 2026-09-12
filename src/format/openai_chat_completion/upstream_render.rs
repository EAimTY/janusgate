//! Upstream-request rendering for the Chat Completions codec: renders an IR
//! request into the `openai_chat_completion` upstream payload.

use crate::{
    format,
    ir::{IrPart, IrRequest, IrResponseFormat, IrRole, IrToolChoice},
};
use serde_json::Value;

/// Renders an IR request into an upstream Chat Completions payload.
pub(crate) fn render_request(ir: &IrRequest, is_streaming: bool) -> Value {
    let mut messages: Vec<Value> = Vec::new();

    if let Some(instructions) = &ir.instructions {
        messages.push(serde_json::json!({"role": "system", "content": instructions}));
    }

    for message in &ir.messages {
        match message.role {
            IrRole::User => {
                let content: Vec<Value> = message
                    .parts
                    .iter()
                    .filter_map(|part| match part {
                        IrPart::Text { text } => {
                            Some(serde_json::json!({"type": "text", "text": text}))
                        }

                        IrPart::Image { image } => {
                            let url = image.to_url();

                            Some(
                                serde_json::json!({"type": "image_url", "image_url": {"url": url}}),
                            )
                        }

                        // The IR type system routes only text and images into
                        // user content.
                        _ => None,
                    })
                    .collect();

                messages.push(serde_json::json!({"role": "user", "content": content}));
            }

            IrRole::Assistant => {
                let texts: Vec<&String> = message
                    .parts
                    .iter()
                    .filter_map(|part| match part {
                        IrPart::Text { text } => Some(text),
                        _ => None,
                    })
                    .collect();

                let tool_calls: Vec<Value> = message
                    .parts
                    .iter()
                    .filter_map(|part| match part {
                        IrPart::ToolCall {
                            call_id,
                            name,
                            arguments,
                            thought_signature,
                        } => {
                            let mut tool_call = serde_json::json!({
                                "id": call_id,
                                "type": "function",
                                "function": {"name": name, "arguments": arguments},
                            });

                            if let Some(signature) = thought_signature {
                                format::json_insert(
                                    &mut tool_call,
                                    "thought_signature",
                                    Value::String(signature.clone()),
                                );
                            }

                            Some(tool_call)
                        }

                        _ => None,
                    })
                    .collect();

                // The Chat Completions wire spec types assistant `content` as
                // `string | null`; parts arrays are not universally accepted
                // by compatible upstreams, and the gateway attaches no
                // per-part semantics to assistant text, so parts join like
                // system instructions do.
                let content = match texts.as_slice() {
                    [] => Value::Null,
                    texts => Value::String(
                        texts
                            .iter()
                            .map(|text| text.as_str())
                            .collect::<Vec<_>>()
                            .join("\n\n"),
                    ),
                };

                let mut message_json = serde_json::json!({"role": "assistant", "content": content});

                if !tool_calls.is_empty() {
                    format::json_insert(&mut message_json, "tool_calls", Value::Array(tool_calls));
                }

                messages.push(message_json);
            }

            IrRole::Tool => {
                for part in &message.parts {
                    if let IrPart::ToolOutput { call_id, content } = part {
                        messages.push(serde_json::json!({
                            "role": "tool",
                            "tool_call_id": call_id,
                            "content": content,
                        }));
                    }
                }
            }
        }
    }

    let mut body = serde_json::json!({
        "model": ir.model,
        "messages": messages,
        "stream": is_streaming,
    });

    if is_streaming {
        format::json_insert(
            &mut body,
            "stream_options",
            serde_json::json!({"include_usage": true}),
        );
    }

    if !ir.tools.is_empty() {
        let tools: Vec<Value> = ir
            .tools
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters,
                    },
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
                serde_json::json!({"type": "function", "function": {"name": name}})
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
            "max_completion_tokens",
            Value::from(max_output_tokens),
        );
    }

    if let Some(response_format) = &ir.response_format {
        let rendered_format = match response_format {
            IrResponseFormat::Text => serde_json::json!({"type": "text"}),
            IrResponseFormat::Json => serde_json::json!({"type": "json_object"}),
            IrResponseFormat::JsonSchema { name, schema } => {
                let json_schema = serde_json::json!({"name": name, "schema": schema});

                serde_json::json!({"type": "json_schema", "json_schema": json_schema})
            }
        };

        format::json_insert(&mut body, "response_format", rendered_format);
    }

    if let Some(effort) = ir.reasoning_effort {
        format::json_insert(
            &mut body,
            "reasoning_effort",
            Value::String(effort.to_openai().to_string()),
        );
    }

    body
}
