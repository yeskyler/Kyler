//! Gemini native generation and conversation-local function continuation.
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use async_openai::error::{OpenAIError, StreamError};
use futures::stream::BoxStream;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::transport::{ModelChunk, native_events, native_http, native_status};

pub(crate) struct Gemini {
    http: reqwest::Client,
    base: String,
    calls: Arc<Mutex<HashMap<String, Value>>>,
    sequence: AtomicU64,
    identity: String,
}

fn invalid(message: &str) -> OpenAIError {
    OpenAIError::StreamError(Box::new(StreamError::EventStream(message.into())))
}

impl Gemini {
    pub(crate) async fn new(base: &str, key: &str) -> anyhow::Result<Self> {
        let mut key = reqwest::header::HeaderValue::from_str(key)?;
        key.set_sensitive(true);
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-goog-api-key", key);
        let base = base.trim_end_matches('/').trim_end_matches("/openai");
        let http = native_http(base, headers).await?;
        static NEXT_SESSION: AtomicU64 = AtomicU64::new(0);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let identity = format!(
            "{timestamp:x}-{:x}",
            NEXT_SESSION.fetch_add(1, Ordering::Relaxed)
        );
        Ok(Self {
            http,
            base: base.into(),
            calls: Default::default(),
            sequence: AtomicU64::new(0),
            identity,
        })
    }

    async fn send(&self, request: &Value, stream: bool) -> Result<reqwest::Response, OpenAIError> {
        let model = request["model"]
            .as_str()
            .ok_or_else(|| invalid("Gemini model is missing"))?;
        let model = model.strip_prefix("models/").unwrap_or(model);
        if model.is_empty()
            || !model
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        {
            return Err(invalid("Invalid Gemini model ID"));
        }
        let method = if stream {
            "streamGenerateContent"
        } else {
            "generateContent"
        };
        let mut builder = self
            .http
            .post(format!("{}/models/{model}:{method}", self.base));
        if stream {
            builder = builder.query(&[("alt", "sse")]);
        }
        let projected = project(request, &*self.calls.lock().await)?;
        let response = builder.json(&projected).send().await?;
        if !response.status().is_success() {
            return Err(native_status(response.status()));
        }
        Ok(response)
    }

    pub(crate) async fn complete(&self, request: Value) -> Result<Value, OpenAIError> {
        let response: Value = self.send(&request, false).await?.json().await?;
        let candidate = &response["candidates"][0];
        let reason = candidate["finishReason"]
            .as_str()
            .ok_or_else(|| invalid("Gemini completion is missing its finish reason"))?;
        let mut turn = Turn::new(
            format!(
                "{}-{}",
                self.identity,
                self.sequence.fetch_add(1, Ordering::Relaxed)
            ),
            self.calls.clone(),
        );
        let (text, _, calls) = turn.parts(candidate).await?;
        let mut message = json!({"role":"assistant","content":text});
        if !calls.is_empty() {
            message["tool_calls"] = json!(calls);
        }
        Ok(json!({
            "id":"gemini","object":"chat.completion","created":0,"model":request["model"],
            "choices":[{"index":0,"message":message,"finish_reason":finish(reason, !calls.is_empty())}],
            "usage":usage(&response),
        }))
    }

    pub(crate) async fn stream(
        &self,
        request: Value,
    ) -> Result<BoxStream<'static, Result<ModelChunk, OpenAIError>>, OpenAIError> {
        use futures::StreamExt;
        let mut events = native_events(self.send(&request, true).await?);
        let mut turn = Turn::new(
            format!(
                "{}-{}",
                self.identity,
                self.sequence.fetch_add(1, Ordering::Relaxed)
            ),
            self.calls.clone(),
        );
        Ok(Box::pin(async_stream::try_stream! {
            let mut finished = None;
            let mut counts = Value::Null;
            while let Some(event) = events.next().await {
                let response = event?;
                if !response["error"].is_null() { Err(invalid("Gemini returned a stream error"))?; }
                if !response["usageMetadata"].is_null() { counts = usage(&response); }
                let candidate = &response["candidates"][0];
                if candidate.is_null() { continue; }
                if finished.is_some() { Err(invalid("Gemini sent a candidate after completion"))?; }
                let (text, reasoning, mut calls) = turn.parts(candidate).await?;
                let first = turn.index - calls.len();
                for (offset, call) in calls.iter_mut().enumerate() {
                    call["index"] = json!(first + offset);
                }
                if !text.is_empty() || !reasoning.is_empty() || !calls.is_empty() {
                    yield decode(json!({"choices":[{"index":0,"delta":{
                        "content":text,"reasoning_content":reasoning,"tool_calls":calls
                    }}]}))?;
                }
                if let Some(reason) = candidate["finishReason"].as_str() {
                    finished = Some(finish(reason, turn.index > 0));
                }
            }
            let reason = finished.ok_or_else(|| invalid("Gemini stream ended before completion"))?;
            yield decode(json!({"choices":[{"index":0,"delta":{},"finish_reason":reason}],"usage":counts}))?;
        }))
    }
}

fn usage(response: &Value) -> Value {
    let counts = &response["usageMetadata"];
    let input = counts["promptTokenCount"].as_u64().unwrap_or(0);
    let output = counts["candidatesTokenCount"]
        .as_u64()
        .unwrap_or(0)
        .saturating_add(counts["thoughtsTokenCount"].as_u64().unwrap_or(0));
    json!({"prompt_tokens":input,"completion_tokens":output,
        "total_tokens":counts["totalTokenCount"].as_u64().unwrap_or(input.saturating_add(output))})
}

fn finish(reason: &str, calls: bool) -> &'static str {
    match reason {
        "STOP" if calls => "tool_calls",
        "STOP" => "stop",
        "MAX_TOKENS" => "length",
        _ => "content_filter",
    }
}

fn decode(value: Value) -> Result<ModelChunk, OpenAIError> {
    serde_json::from_value(value).map_err(|_| invalid("Invalid Gemini stream payload"))
}

struct Turn {
    sequence: String,
    index: usize,
    calls: Arc<Mutex<HashMap<String, Value>>>,
}
impl Turn {
    fn new(sequence: String, calls: Arc<Mutex<HashMap<String, Value>>>) -> Self {
        Self {
            sequence,
            index: 0,
            calls,
        }
    }
    async fn parts(
        &mut self,
        candidate: &Value,
    ) -> Result<(String, String, Vec<Value>), OpenAIError> {
        let mut text = String::new();
        let mut reasoning = String::new();
        let mut calls = Vec::new();
        for part in candidate["content"]["parts"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if let Some(call) = part["functionCall"].as_object() {
                let id = format!("gemini-{}-{}", self.sequence, self.index);
                self.index += 1;
                let name = call
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid("Gemini tool name missing"))?;
                let arguments = call.get("args").cloned().unwrap_or_else(|| json!({}));
                self.calls.lock().await.insert(id.clone(), part.clone());
                calls.push(json!({"id":id,"type":"function","function":{"name":name,"arguments":arguments.to_string()}}));
            } else if let Some(value) = part["text"].as_str() {
                if part["thought"] == true {
                    reasoning.push_str(value);
                } else {
                    text.push_str(value);
                }
            }
        }
        Ok((text, reasoning, calls))
    }
}

fn project(request: &Value, native_calls: &HashMap<String, Value>) -> Result<Value, OpenAIError> {
    let mut system = Vec::new();
    let mut contents: Vec<Value> = Vec::new();
    let mut names = HashMap::new();
    for message in request["messages"]
        .as_array()
        .ok_or_else(|| invalid("Gemini messages are missing"))?
    {
        let role = message["role"].as_str().unwrap_or_default();
        if matches!(role, "system" | "developer") {
            if let Some(text) = message["content"].as_str() {
                system.push(json!({"text":text}));
            }
            continue;
        }
        let mut parts = Vec::new();
        if role == "tool" {
            let id = message["tool_call_id"]
                .as_str()
                .ok_or_else(|| invalid("Gemini tool result ID is missing"))?;
            let name = names
                .get(id)
                .ok_or_else(|| invalid("Gemini tool result has no matching call"))?;
            if let Some(native) = native_calls.get(id) {
                let mut response = json!({"name":name,"response":{"output":message["content"]}});
                if let Some(id) = native["functionCall"]["id"].as_str() {
                    response["id"] = json!(id);
                }
                parts.push(json!({"functionResponse":response}));
            } else {
                let output = message["content"].as_str().unwrap_or_default();
                parts.push(json!({"text":format!("Historical tool result ({name}): {output}")}));
            }
        } else if let Some(text) = message["content"].as_str().filter(|text| !text.is_empty()) {
            parts.push(json!({"text":text}));
        } else {
            for part in message["content"].as_array().into_iter().flatten() {
                match part["type"].as_str() {
                    Some("text") => parts.push(json!({"text":part["text"]})),
                    Some("image_url") => {
                        let (mime, data) = part["image_url"]["url"]
                            .as_str()
                            .and_then(|url| url.strip_prefix("data:"))
                            .and_then(|url| url.split_once(";base64,"))
                            .ok_or_else(|| {
                                invalid("Gemini image must be a prepared inline image")
                            })?;
                        parts.push(json!({"inlineData":{"mimeType":mime,"data":data}}));
                    }
                    _ => return Err(invalid("Unsupported Gemini input content")),
                }
            }
        }
        for call in message["tool_calls"].as_array().into_iter().flatten() {
            let id = call["id"]
                .as_str()
                .ok_or_else(|| invalid("Gemini tool ID is missing"))?;
            let name = call["function"]["name"]
                .as_str()
                .ok_or_else(|| invalid("Gemini tool name is missing"))?;
            names.insert(id, name);
            if let Some(part) = native_calls.get(id) {
                parts.push(part.clone());
            } else {
                // Past executions lack the provider's signed function blocks.
                parts.push(json!({"text":format!("Historical tool call: {name}")}));
            }
        }
        if parts.is_empty() {
            continue;
        }
        let role = if role == "assistant" { "model" } else { "user" };
        if let Some(previous) = contents.last_mut().filter(|last| last["role"] == role) {
            previous["parts"]
                .as_array_mut()
                .expect("projected parts are an array")
                .extend(parts);
        } else {
            contents.push(json!({"role":role,"parts":parts}));
        }
    }
    let mut output = json!({"contents":contents});
    if !system.is_empty() {
        output["systemInstruction"] = json!({"parts":system});
    }
    let limit = request["max_completion_tokens"]
        .as_u64()
        .or_else(|| request["max_tokens"].as_u64());
    if let Some(limit) = limit {
        output["generationConfig"] = json!({"maxOutputTokens":limit});
    }
    if let Some(tools) = request["tools"]
        .as_array()
        .filter(|tools| !tools.is_empty())
    {
        let declarations: Vec<_> = tools.iter().map(|tool| {
            let function = &tool["function"];
            json!({"name":function["name"],"description":function["description"],"parametersJsonSchema":function["parameters"]})
        }).collect();
        output["tools"] = json!([{"functionDeclarations":declarations}]);
    }
    if request["reasoning_effort"]
        .as_str()
        .is_some_and(|effort| !matches!(effort, "none" | "off"))
    {
        return Err(OpenAIError::InvalidArgument("Gemini native thinking levels are not configured for this model. Set reasoning to Off.".into()));
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_function_signatures_are_replayed_with_matching_results() {
        let mut native = HashMap::new();
        native.insert("call".into(), json!({"functionCall":{"id":"remote","name":"read","args":{}},"thoughtSignature":"opaque"}));
        let projected = project(&json!({"messages":[
            {"role":"user","content":"Read"},
            {"role":"assistant","tool_calls":[{"id":"call","function":{"name":"read","arguments":"{}"}}]},
            {"role":"tool","tool_call_id":"call","content":"Result"}
        ]}), &native).unwrap();
        assert_eq!(
            projected["contents"][1]["parts"][0]["thoughtSignature"],
            "opaque"
        );
        assert_eq!(
            projected["contents"][2]["parts"][0]["functionResponse"]["id"],
            "remote"
        );
        assert_eq!(
            projected["contents"][2]["parts"][0]["functionResponse"]["name"],
            "read"
        );
    }
}
