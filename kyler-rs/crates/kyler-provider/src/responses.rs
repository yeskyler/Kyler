//! Stateless Responses mapping. Opaque reasoning stays private to one execution.
use crate::transport::{ModelChunk, ModelSession};

use std::collections::{BTreeMap, HashMap};

use async_openai::error::OpenAIError;
use futures::{StreamExt, stream::BoxStream};
use serde_json::{Value, json};

#[derive(Default)]
pub(crate) struct Continuation {
    outputs: HashMap<String, Vec<Value>>,
}
fn invalid(message: &str) -> OpenAIError {
    OpenAIError::StreamError(Box::new(async_openai::error::StreamError::EventStream(
        message.into(),
    )))
}
fn output_text(items: &[Value]) -> String {
    items
        .iter()
        .filter(|i| i["type"] == "message")
        .flat_map(|i| i["content"].as_array().into_iter().flatten())
        .filter_map(|p| p["text"].as_str().or_else(|| p["refusal"].as_str()))
        .collect()
}
impl Continuation {
    fn remember(&mut self, response: &Value) {
        if let Some(output) = response["output"].as_array() {
            if let Some(id) = output
                .iter()
                .find(|i| i["type"] == "function_call")
                .and_then(|i| i["call_id"].as_str())
            {
                self.outputs.insert(id.into(), output.clone());
            }
        }
    }
    fn request(&self, request: &Value) -> Result<Value, OpenAIError> {
        let mut input = Vec::new();
        for message in request["messages"]
            .as_array()
            .ok_or_else(|| invalid("Model messages are missing"))?
        {
            let role = message["role"].as_str().unwrap_or("user");
            if role == "tool" {
                input.push(json!({"type":"function_call_output","call_id":message["tool_call_id"],"output":message["content"]}));
                continue;
            }
            let calls = message["tool_calls"].as_array();
            if let Some(output) = calls
                .and_then(|c| c.first())
                .and_then(|c| c["id"].as_str())
                .and_then(|id| self.outputs.get(id))
            {
                // Text and calls are separate core events. Replace their text tail with
                // the original provider output, preserving reasoning/item order exactly.
                let text = output_text(output);
                let mut count = 0;
                let mut tail = String::new();
                for item in input.iter().rev() {
                    if item["role"] != "assistant" {
                        break;
                    }
                    let Some(part) = item["content"].as_str() else {
                        break;
                    };
                    tail.insert_str(0, part);
                    count += 1;
                    if tail == text {
                        input.truncate(input.len() - count);
                        break;
                    }
                    if tail.len() >= text.len() {
                        break;
                    }
                }
                input.extend(output.clone());
                continue;
            }
            if !message["content"].is_null() {
                let content = if let Some(parts) = message["content"].as_array() {
                    Value::Array(parts.iter().map(|part| match part["type"].as_str() {
                        Some("text") => json!({"type":if role=="assistant" {"output_text"} else {"input_text"},"text":part["text"]}),
                        Some("image_url") => json!({"type":"input_image","image_url":part["image_url"]["url"],"detail":part["image_url"].get("detail").cloned().unwrap_or(json!("auto"))}),
                        _ => Value::Null,
                    }).collect())
                } else {
                    message["content"].clone()
                };
                if content
                    .as_array()
                    .is_some_and(|p| p.iter().any(Value::is_null))
                {
                    return Err(invalid("Unsupported Responses input content"));
                }
                input.push(json!({"role":role,"content":content}));
            }
            for call in calls.into_iter().flatten() {
                input.push(json!({"type":"function_call","call_id":call["id"],"name":call["function"]["name"],"arguments":call["function"]["arguments"]}));
            }
        }
        let mut result = json!({"model":request["model"],"input":input,"store":false,"include":["reasoning.encrypted_content"]});
        if let Some(limit) = request
            .get("max_completion_tokens")
            .or_else(|| request.get("max_tokens"))
        {
            result["max_output_tokens"] = limit.clone();
        }
        if let Some(effort) = request.get("reasoning_effort") {
            result["reasoning"] = json!({"effort":effort});
        }
        if let Some(tools) = request["tools"].as_array() {
            let mut projected = Vec::with_capacity(tools.len());
            for tool in tools {
                if tool["type"] != "function" || !tool["function"]["name"].is_string() {
                    return Err(invalid("Unsupported Responses tool definition"));
                }
                let mut function = json!({"type":"function","name":tool["function"]["name"],"parameters":tool["function"]["parameters"],"strict":false});
                if let Some(description) = tool["function"]
                    .get("description")
                    .filter(|v| v.is_string())
                {
                    function["description"] = description.clone();
                }
                projected.push(function);
            }
            result["tools"] = Value::Array(projected);
        }
        for key in ["parallel_tool_calls", "stream", "temperature", "top_p"] {
            if let Some(value) = request.get(key) {
                result[key] = value.clone();
            }
        }
        if let Some(choice) = request.get("tool_choice") {
            result["tool_choice"] = if choice.is_string() {
                choice.clone()
            } else {
                json!({"type":"function","name":choice["function"]["name"]})
            };
        }
        Ok(result)
    }
}
fn usage(response: &Value) -> Value {
    let usage = &response["usage"];
    json!({"prompt_tokens":usage["input_tokens"].as_u64().unwrap_or(0),"completion_tokens":usage["output_tokens"].as_u64().unwrap_or(0),"total_tokens":usage["total_tokens"].as_u64().unwrap_or(0)})
}
fn chunk(
    delta: Value,
    finish: Option<&str>,
    usage: Option<Value>,
) -> Result<ModelChunk, OpenAIError> {
    serde_json::from_value(
        json!({"choices":[{"index":0,"delta":delta,"finish_reason":finish}],"usage":usage}),
    )
    .map_err(|_| invalid("Invalid Responses event"))
}
// Keep the provider cause private; the host exposes only classified diagnostics.
fn stream_failure(error: &Value) -> OpenAIError {
    OpenAIError::StreamError(Box::new(async_openai::error::StreamError::EventStream(
        json!({"error":error}).to_string(),
    )))
}
fn check(response: &Value) -> Result<(), OpenAIError> {
    if response["status"] == "failed" {
        return Err(stream_failure(&response["error"]));
    }
    if response["status"] != "completed" {
        return Err(invalid(
            "Responses request ended without a completed result",
        ));
    }
    Ok(())
}
pub(crate) async fn complete(session: &ModelSession, request: Value) -> Result<Value, OpenAIError> {
    let projected = session.continuation.lock().await.request(&request)?;
    let response: Value = session.sdk.responses().create_byot(projected).await?;
    check(&response)?;
    session.continuation.lock().await.remember(&response);
    let output = response["output"]
        .as_array()
        .ok_or_else(|| invalid("Responses output is missing"))?;
    let reasoning: String = output
        .iter()
        .filter(|item| item["type"] == "reasoning")
        .flat_map(|item| item["summary"].as_array().into_iter().flatten())
        .filter_map(|part| part["text"].as_str())
        .collect();
    let calls:Vec<_>=output.iter().filter(|i|i["type"]=="function_call").map(|i|json!({"id":i["call_id"],"type":"function","function":{"name":i["name"],"arguments":i["arguments"]}})).collect();
    Ok(
        json!({"id":response["id"],"object":"chat.completion","created":0,"model":request["model"],
        "choices":[{"index":0,"message":{"role":"assistant","content":output_text(output),"reasoning_content":reasoning,"tool_calls":if calls.is_empty(){Value::Null}else{json!(calls)},"refusal":null},"finish_reason":if calls.is_empty(){"stop"}else{"tool_calls"}}],"usage":usage(&response)}),
    )
}

pub(crate) async fn stream(
    session: &ModelSession,
    request: Value,
) -> Result<BoxStream<'static, Result<ModelChunk, OpenAIError>>, OpenAIError> {
    let projected = session.continuation.lock().await.request(&request)?;
    let mut source = session
        .sdk
        .responses()
        .create_stream_byot::<_, Value>(projected)
        .await?;
    let continuation = session.continuation.clone();
    Ok(Box::pin(async_stream::try_stream! {
        let mut calls: BTreeMap<u64, (String, String, String)> = BTreeMap::new();
        let mut completed = false;
        while let Some(event) = source.next().await {
            let event = event?;
            if completed { Err(invalid("Responses sent data after completion"))?; }
            match event["type"].as_str().unwrap_or_default() {
                "response.output_text.delta" | "response.refusal.delta" => { yield chunk(json!({"content":event["delta"]}), None, None)?; }
                "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => { yield chunk(json!({"reasoning":event["delta"]}), None, None)?; }
                "response.output_item.added" if event["item"]["type"]=="function_call" => {
                    let index=event["output_index"].as_u64().ok_or_else(||invalid("Missing Responses output index"))?;
                    let item=&event["item"];
                    let id=item["call_id"].as_str().filter(|s|!s.is_empty()).ok_or_else(||invalid("Missing function call ID"))?;
                    let name=item["name"].as_str().filter(|s|!s.is_empty()).ok_or_else(||invalid("Missing function name"))?;
                    if calls.insert(index,(id.into(),name.into(),String::new())).is_some() { Err(invalid("Duplicate function call index"))?; }
                    yield chunk(json!({"tool_calls":[{"index":index,"id":id,"type":"function","function":{"name":name,"arguments":""}}]}),None,None)?;
                }
                "response.function_call_arguments.delta" => {
                    let index=event["output_index"].as_u64().ok_or_else(||invalid("Missing function argument index"))?;
                    let delta=event["delta"].as_str().ok_or_else(||invalid("Invalid function argument delta"))?;
                    let call=calls.get_mut(&index).ok_or_else(||invalid("Function arguments arrived before the call"))?;
                    call.2.push_str(delta);
                    yield chunk(json!({"tool_calls":[{"index":index,"function":{"arguments":delta}}]}),None,None)?;
                }
                "response.output_item.done" if event["item"]["type"]=="function_call" => {
                    let index=event["output_index"].as_u64().ok_or_else(||invalid("Missing function completion index"))?;
                    let item=&event["item"];
                    let call=calls.get(&index).ok_or_else(||invalid("Function completion arrived before the call"))?;
                    if item["call_id"]!=call.0 || item["name"]!=call.1 || item["arguments"]!=call.2 { Err(invalid("Function completion does not match its streamed arguments"))?; }
                }
                "response.completed" => {
                    let response=&event["response"];
                    check(response)?;
                    let outputs=response["output"].as_array().ok_or_else(||invalid("Responses output is missing"))?;
                    let output_calls:Vec<_>=outputs.iter().filter(|i|i["type"]=="function_call").collect();
                    if output_calls.len()!=calls.len() || output_calls.iter().any(|i|!calls.values().any(|c| i["call_id"]==c.0 && i["name"]==c.1 && i["arguments"]==c.2)) { Err(invalid("Incomplete Responses function calls"))?; }
                    continuation.lock().await.remember(response);
                    yield chunk(json!({}),Some(if calls.is_empty(){"stop"}else{"tool_calls"}),Some(usage(response)))?;
                    completed=true;
                }
                "response.failed" => { Err(stream_failure(&event["response"]["error"]))?; }
                "error" => { Err(stream_failure(&event))?; }
                "response.incomplete" => { Err(OpenAIError::StreamError(Box::new(async_openai::error::StreamError::EventStream("Responses stream was incomplete".into()))))?; }
                _ => {}
            }
        }
        if !completed { Err(invalid("Responses stream ended before completion"))?; }
    }))
}
