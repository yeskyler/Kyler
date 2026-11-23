//! Anthropic Messages transport normalized to the agent's tool-call envelope.
use crate::transport::ModelChunk;

use async_openai::error::{OpenAIError, StreamError};
use futures::stream::BoxStream;
use serde_json::{Value, json};

pub(crate) struct Messages {
    http: reqwest::Client,
    url: String,
}

fn invalid(message: &str) -> OpenAIError {
    OpenAIError::StreamError(Box::new(StreamError::EventStream(message.into())))
}

impl Messages {
    pub(crate) async fn new(base: &str, key: &str) -> anyhow::Result<Self> {
        let mut key = reqwest::header::HeaderValue::from_str(key)?;
        key.set_sensitive(true);
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-api-key", key);
        headers.insert("anthropic-version", "2023-06-01".parse()?);
        let http = crate::transport::native_http(base, headers).await?;
        Ok(Self {
            http,
            url: format!("{}/messages", base.trim_end_matches('/')),
        })
    }

    async fn send(&self, request: &Value, stream: bool) -> Result<reqwest::Response, OpenAIError> {
        let projected = project(request, stream)?;
        let response = self.http.post(&self.url).json(&projected).send().await?;
        if !response.status().is_success() {
            return Err(crate::transport::native_status(response.status()));
        }
        Ok(response)
    }

    pub(crate) async fn complete(&self, request: Value) -> Result<Value, OpenAIError> {
        let response = self.send(&request, false).await?.json::<Value>().await?;
        normalize(&response, &request)
    }

    pub(crate) async fn stream(
        &self,
        request: Value,
    ) -> Result<BoxStream<'static, Result<ModelChunk, OpenAIError>>, OpenAIError> {
        use futures::StreamExt;
        let mut events = crate::transport::native_events(self.send(&request, true).await?);
        Ok(Box::pin(async_stream::try_stream! {
            let mut input = 0;
            let mut stopped = false;
            let mut finish_event = None;
            let mut open_tools = std::collections::HashMap::new();
            while let Some(event) = events.next().await {
                    let event = event?;
                    let index = event["index"].as_u64().unwrap_or(0);
                    if stopped {
                        Err(invalid("Anthropic sent data after message_stop"))?;
                    }
                    let output = match event["type"].as_str() {
                        Some("message_start") => {
                            input = input_tokens(&event["message"]["usage"]);
                            continue;
                        }
                        Some("content_block_start") if event["content_block"]["type"] == "tool_use" => {
                            let block = &event["content_block"];
                            let id = block["id"].as_str().ok_or_else(|| invalid("Anthropic tool ID missing"))?;
                            let name = block["name"].as_str().ok_or_else(|| invalid("Anthropic tool name missing"))?;
                            let initial = event["content_block"]["input"].as_object()
                                .filter(|input| !input.is_empty()).map(|input| json!(input).to_string()).unwrap_or_default();
                            if open_tools.insert(index, !initial.is_empty()).is_some() {
                                Err(invalid("Duplicate Anthropic tool block"))?;
                            }
                            json!({"tool_calls":[{"index":index,"id":id,"type":"function","function":{"name":name,"arguments":initial}}]})
                        }
                        Some("content_block_start") if event["content_block"]["type"] == "text" => json!({"content":event["content_block"]["text"]}),
                        Some("content_block_delta") => match event["delta"]["type"].as_str() {
                            Some("text_delta") => json!({"content":event["delta"]["text"]}),
                            Some("input_json_delta") => {
                                let arguments = event["delta"]["partial_json"].as_str().ok_or_else(|| invalid("Invalid Anthropic tool delta"))?;
                                *open_tools.get_mut(&index).ok_or_else(|| invalid("Anthropic tool delta has no open block"))? |= !arguments.is_empty();
                                json!({"tool_calls":[{"index":index,"function":{"arguments":arguments}}]})
                            }
                            _ => continue,
                        },
                        Some("content_block_stop") => {
                            if open_tools.remove(&index) == Some(false) {
                                json!({"tool_calls":[{"index":index,"function":{"arguments":"{}"}}]})
                            } else {
                                continue;
                            }
                        }
                        Some("message_delta") => {
                            let Some(reason) = event["delta"]["stop_reason"].as_str() else { continue };
                            let count = event["usage"]["output_tokens"].as_u64().unwrap_or(0);
                            finish_event = Some(json!({"choices":[{"index":0,"delta":{},"finish_reason":finish(reason)}],"usage":usage(input,count)}));
                            continue;
                        }
                        Some("message_stop") => {
                            if !open_tools.is_empty() {
                                Err(invalid("Anthropic stream contains incomplete tool blocks"))?;
                            }
                            stopped = true;
                            yield chunk_value(finish_event.take().ok_or_else(|| invalid("Anthropic stop reason missing"))?)?;
                            continue;
                        }
                        Some("error") => Err(invalid("Anthropic returned a stream error"))?,
                        _ => continue,
                    };
                    yield chunk_value(json!({"choices":[{"index":0,"delta":output}]}))?;
            }
            if !stopped {
                Err(invalid("Anthropic stream ended before completion"))?;
            }
        }))
    }
}

fn chunk_value(value: Value) -> Result<ModelChunk, OpenAIError> {
    serde_json::from_value(value).map_err(|_| invalid("Invalid Anthropic stream payload"))
}

fn usage(input: u64, output: u64) -> Value {
    json!({"prompt_tokens":input,"completion_tokens":output,"total_tokens":input.saturating_add(output)})
}

fn input_tokens(usage: &Value) -> u64 {
    [
        "input_tokens",
        "cache_creation_input_tokens",
        "cache_read_input_tokens",
    ]
    .into_iter()
    .filter_map(|key| usage[key].as_u64())
    .fold(0, u64::saturating_add)
}

fn finish(reason: &str) -> &'static str {
    match reason {
        "tool_use" => "tool_calls",
        "max_tokens" => "length",
        "refusal" => "content_filter",
        _ => "stop",
    }
}

fn normalize(response: &Value, request: &Value) -> Result<Value, OpenAIError> {
    let mut text = String::new();
    let mut calls = Vec::new();
    for block in response["content"]
        .as_array()
        .ok_or_else(|| invalid("Anthropic response is missing content"))?
    {
        match block["type"].as_str() {
            Some("text") => text.push_str(block["text"].as_str().unwrap_or_default()),
            Some("tool_use") => calls.push(json!({"id":block["id"],"type":"function","function":{"name":block["name"],"arguments":block["input"].to_string()}})),
            _ => {}
        }
    }
    let mut message = json!({"role":"assistant","content":text});
    if !calls.is_empty() {
        message["tool_calls"] = json!(calls);
    }
    let reason = response["stop_reason"]
        .as_str()
        .ok_or_else(|| invalid("Anthropic response is missing a stop reason"))?;
    Ok(json!({
        "id":response["id"], "object":"chat.completion", "created":0, "model":request["model"],
        "choices":[{"index":0,"message":message,"finish_reason":finish(reason)}],
        "usage":usage(input_tokens(&response["usage"]),response["usage"]["output_tokens"].as_u64().unwrap_or(0))
    }))
}

fn project(request: &Value, stream: bool) -> Result<Value, OpenAIError> {
    let mut messages: Vec<Value> = Vec::new();
    let mut system = Vec::new();
    for message in request["messages"]
        .as_array()
        .ok_or_else(|| invalid("Model messages are missing"))?
    {
        let role = message["role"].as_str().unwrap_or_default();
        if role == "system" || role == "developer" {
            if let Some(content) = message["content"].as_str() {
                system.push(content);
            }
            continue;
        }
        let mut blocks = Vec::new();
        if role == "tool" {
            blocks.push(json!({"type":"tool_result","tool_use_id":message["tool_call_id"],"content":message["content"]}));
        } else {
            if let Some(text) = message["content"].as_str().filter(|text| !text.is_empty()) {
                blocks.push(json!({"type":"text","text":text}));
            }
            if let Some(parts) = message["content"].as_array() {
                for part in parts {
                    match part["type"].as_str() {
                        Some("text") => blocks.push(json!({"type":"text","text":part["text"]})),
                        Some("image_url") => {
                            let url = part["image_url"]["url"]
                                .as_str()
                                .ok_or_else(|| invalid("Missing Anthropic image URL"))?;
                            let (media_type, data) = url
                                .strip_prefix("data:")
                                .and_then(|url| url.split_once(";base64,"))
                                .ok_or_else(|| {
                                    invalid("Anthropic image must be a prepared inline image")
                                })?;
                            blocks.push(json!({"type":"image","source":{"type":"base64","media_type":media_type,"data":data}}));
                        }
                        _ => return Err(invalid("Unsupported Anthropic input content")),
                    }
                }
            }
            for call in message["tool_calls"].as_array().into_iter().flatten() {
                let arguments: Value =
                    serde_json::from_str(call["function"]["arguments"].as_str().unwrap_or("{}"))
                        .map_err(|_| invalid("Invalid Anthropic tool arguments"))?;
                blocks.push(json!({"type":"tool_use","id":call["id"],"name":call["function"]["name"],"input":arguments}));
            }
        }
        if blocks.is_empty() {
            continue;
        }
        let role = if role == "tool" { "user" } else { role };
        if let Some(previous) = messages.last_mut().filter(|last| last["role"] == role) {
            previous["content"]
                .as_array_mut()
                .expect("projected content is an array")
                .extend(blocks);
        } else {
            messages.push(json!({"role":role,"content":blocks}));
        }
    }
    let mut projected = json!({
        "model": request["model"], "messages": messages, "stream": stream,
        "max_tokens": request["max_tokens"].as_u64().or_else(||request["max_completion_tokens"].as_u64()).unwrap_or(8192),
    });
    if !system.is_empty() {
        projected["system"] = json!(system.join("\n\n"));
    }
    if let Some(tools) = request["tools"]
        .as_array()
        .filter(|tools| !tools.is_empty())
    {
        projected["tools"] = json!(tools.iter().map(|tool| {
            let f = &tool["function"];
            json!({"name":f["name"],"description":f["description"].as_str().unwrap_or_default(),"input_schema":f["parameters"]})
        }).collect::<Vec<_>>());
        projected["tool_choice"] = json!({"type":"auto","disable_parallel_tool_use":request["parallel_tool_calls"] == false});
    }
    if request["reasoning_effort"]
        .as_str()
        .is_some_and(|effort| !matches!(effort, "none" | "off"))
    {
        return Err(OpenAIError::InvalidArgument(
            "Claude native thinking is not supported by this adapter yet. Set reasoning to Off."
                .into(),
        ));
    }
    Ok(projected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_results_are_batched_with_native_ids_and_system_is_separate() {
        let request = json!({"model":"fixture","messages":[
            {"role":"system","content":"System"},
            {"role":"user","content":"Run two tools"},
            {"role":"assistant","tool_calls":[
                {"id":"call-a","function":{"name":"first","arguments":"{}"}},
                {"id":"call-b","function":{"name":"second","arguments":"{}"}}
            ]},
            {"role":"tool","tool_call_id":"call-a","content":"A"},
            {"role":"tool","tool_call_id":"call-b","content":"B"}
        ]});
        let value = project(&request, true).unwrap();
        assert_eq!(value["system"], "System");
        assert_eq!(value["messages"][1]["content"][0]["type"], "tool_use");
        assert_eq!(value["messages"][2]["content"][0]["tool_use_id"], "call-a");
        assert_eq!(value["messages"][2]["content"][1]["tool_use_id"], "call-b");
    }
}
