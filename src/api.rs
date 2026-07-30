use std::time::Duration;

use futures_util::{StreamExt, TryStreamExt};
use reqwest::{header, Client, Response, StatusCode};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    config::{AppConfig, SUPPORTED_OPENAI_MODELS},
    event::ApiEvent,
    model::{RequestMessage, TokenUsage},
    proxy,
};

const MAX_ERROR_BODY_BYTES: usize = 16 * 1024;
const MAX_ERROR_MESSAGE_CHARS: usize = 240;
const MAX_SSE_EVENT_BYTES: usize = 1024 * 1024;
const MAX_STREAM_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_STREAM_TEXT_BYTES: usize = 8 * 1024 * 1024;
const MAX_SSE_BLOCKS: usize = 32 * 1024;

#[derive(Clone)]
pub struct ApiClient {
    client: Client,
    endpoint: String,
    api_key: String,
    temperature: f32,
    max_tokens: u32,
    timeout: Duration,
}

#[derive(Clone, Debug)]
pub struct ApiRequest {
    pub id: u64,
    pub model: String,
    pub messages: Vec<RequestMessage>,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [RequestMessage],
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_completion_tokens: Option<u32>,
    stream: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ModelApiProfile {
    reasoning: bool,
    modern_token_limit: bool,
}

impl ModelApiProfile {
    fn for_model(model: &str) -> Self {
        let reasoning = is_reasoning_model(model);
        let modern_token_limit = reasoning
            || SUPPORTED_OPENAI_MODELS.iter().any(|alias| {
                model == *alias
                    || model
                        .strip_prefix(alias)
                        .is_some_and(|suffix| suffix.starts_with("-20"))
            });
        Self {
            reasoning,
            modern_token_limit,
        }
    }
}

fn is_reasoning_model(model: &str) -> bool {
    model == "gpt-5"
        || model.starts_with("gpt-5-")
        || model.starts_with("gpt-5.")
        || ["o1", "o3", "o4"]
            .iter()
            .any(|family| model == *family || model.starts_with(&format!("{family}-")))
}

fn adapt_messages(messages: &[RequestMessage], profile: ModelApiProfile) -> Vec<RequestMessage> {
    messages
        .iter()
        .cloned()
        .map(|mut message| {
            if profile.reasoning && message.role == "system" {
                message.role = "developer";
            }
            message
        })
        .collect()
}

fn chat_request<'a>(
    model: &'a str,
    messages: &'a [RequestMessage],
    temperature: f32,
    max_tokens: u32,
) -> ChatRequest<'a> {
    let profile = ModelApiProfile::for_model(model);
    ChatRequest {
        model,
        messages,
        temperature: (!profile.reasoning).then_some(temperature),
        max_tokens: (!profile.modern_token_limit).then_some(max_tokens),
        max_completion_tokens: profile.modern_token_limit.then_some(max_tokens),
        stream: true,
    }
}

#[derive(Debug, Error)]
pub enum StreamError {
    #[error("invalid UTF-8 in SSE stream")]
    InvalidUtf8,
    #[error("invalid JSON in SSE event")]
    InvalidJson,
    #[error("SSE event exceeds the 1 MiB limit")]
    EventTooLarge,
    #[error("stream response exceeds the 16 MiB limit")]
    ResponseTooLarge,
    #[error("stream text exceeds the 8 MiB limit")]
    TextTooLarge,
    #[error("stream exceeds the 32768 SSE block limit")]
    TooManyBlocks,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SseEvent {
    Data(String),
    Done,
}

#[derive(Default)]
pub struct SseDecoder {
    buffer: Vec<u8>,
    search_offset: usize,
    blocks_seen: usize,
}

impl SseDecoder {
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<SseEvent>, StreamError> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        let mut consumed = 0;
        let mut search_from = self.search_offset.min(self.buffer.len());

        while let Some((separator_index, separator_len)) =
            find_event_separator(&self.buffer[search_from..])
        {
            let event_end = search_from + separator_index;
            if event_end.saturating_sub(consumed) > MAX_SSE_EVENT_BYTES {
                return Err(StreamError::EventTooLarge);
            }
            self.record_block()?;
            if let Some(event) = parse_sse_block(&self.buffer[consumed..event_end])? {
                events.push(event);
            }
            consumed = event_end + separator_len;
            search_from = consumed;
        }

        if self.buffer.len().saturating_sub(consumed) > MAX_SSE_EVENT_BYTES {
            return Err(StreamError::EventTooLarge);
        }
        if consumed > 0 {
            self.buffer.drain(..consumed);
        }
        // A separator can straddle two chunks. CRLF needs at most the final
        // three bytes to be checked again after the next append.
        self.search_offset = self.buffer.len().saturating_sub(3);

        Ok(events)
    }

    pub fn finish(&mut self) -> Result<Vec<SseEvent>, StreamError> {
        if self.buffer.is_empty() {
            return Ok(Vec::new());
        }
        if self.buffer.len() > MAX_SSE_EVENT_BYTES {
            return Err(StreamError::EventTooLarge);
        }
        self.record_block()?;
        let raw = std::mem::take(&mut self.buffer);
        self.search_offset = 0;
        Ok(parse_sse_block(&raw)?.into_iter().collect())
    }

    fn record_block(&mut self) -> Result<(), StreamError> {
        self.blocks_seen = checked_total(
            self.blocks_seen,
            1,
            MAX_SSE_BLOCKS,
            StreamError::TooManyBlocks,
        )?;
        Ok(())
    }
}

#[derive(Default)]
struct StreamBudget {
    response_bytes: usize,
    text_bytes: usize,
}

impl StreamBudget {
    fn record_response_bytes(&mut self, bytes: usize) -> Result<(), StreamError> {
        self.response_bytes = checked_total(
            self.response_bytes,
            bytes,
            MAX_STREAM_RESPONSE_BYTES,
            StreamError::ResponseTooLarge,
        )?;
        Ok(())
    }

    fn record_text_bytes(&mut self, bytes: usize) -> Result<(), StreamError> {
        self.text_bytes = checked_total(
            self.text_bytes,
            bytes,
            MAX_STREAM_TEXT_BYTES,
            StreamError::TextTooLarge,
        )?;
        Ok(())
    }
}

fn checked_total(
    current: usize,
    added: usize,
    limit: usize,
    error: StreamError,
) -> Result<usize, StreamError> {
    current
        .checked_add(added)
        .filter(|total| *total <= limit)
        .ok_or(error)
}

fn find_event_separator(buffer: &[u8]) -> Option<(usize, usize)> {
    let mut index = 0;
    while index < buffer.len() {
        if buffer[index..].starts_with(b"\n\n") {
            return Some((index, 2));
        }
        if buffer[index..].starts_with(b"\r\n\r\n") {
            return Some((index, 4));
        }
        index += 1;
    }
    None
}

fn parse_sse_block(raw: &[u8]) -> Result<Option<SseEvent>, StreamError> {
    let text = std::str::from_utf8(raw).map_err(|_| StreamError::InvalidUtf8)?;
    let mut data_lines = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        if let Some(value) = line.strip_prefix("data:") {
            data_lines.push(value.strip_prefix(' ').unwrap_or(value));
        }
    }

    if data_lines.is_empty() {
        return Ok(None);
    }
    let data = data_lines.join("\n");
    if data.trim() == "[DONE]" {
        Ok(Some(SseEvent::Done))
    } else {
        Ok(Some(SseEvent::Data(data)))
    }
}

#[derive(Debug, Deserialize)]
struct StreamPayload {
    #[serde(default)]
    choices: Vec<Choice>,
    usage: Option<UsagePayload>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    delta: Option<DeltaPayload>,
}

#[derive(Debug, Deserialize)]
struct DeltaPayload {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UsagePayload {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

#[derive(Debug, Default, Eq, PartialEq)]
struct ParsedPayload {
    delta: Option<String>,
    usage: Option<TokenUsage>,
}

fn parse_stream_payload(data: &str) -> Result<ParsedPayload, StreamError> {
    let payload: StreamPayload =
        serde_json::from_str(data).map_err(|_| StreamError::InvalidJson)?;
    let delta = payload
        .choices
        .first()
        .and_then(|choice| choice.delta.as_ref())
        .and_then(|delta| delta.content.clone())
        .filter(|content| !content.is_empty());
    let usage = payload.usage.map(|usage| TokenUsage {
        prompt_tokens: usage.prompt_tokens,
        completion_tokens: usage.completion_tokens,
        total_tokens: usage.total_tokens,
    });
    Ok(ParsedPayload { delta, usage })
}

impl ApiClient {
    pub fn new(config: &AppConfig) -> anyhow::Result<Self> {
        let builder = Client::builder()
            .connect_timeout(Duration::from_secs(15).min(config.timeout))
            .user_agent(format!("tinychat/{}", env!("CARGO_PKG_VERSION")));
        let client = proxy::configure_from_env(builder)?.build()?;
        Ok(Self {
            client,
            endpoint: config.chat_completions_url(),
            api_key: config.api_key.clone(),
            temperature: config.temperature,
            max_tokens: config.max_tokens,
            timeout: config.timeout,
        })
    }

    pub async fn run(
        &self,
        request: ApiRequest,
        cancellation: CancellationToken,
        events: mpsc::Sender<ApiEvent>,
    ) {
        let request_id = request.id;
        if events.send(ApiEvent::Started { request_id }).await.is_err() {
            return;
        }

        let profile = ModelApiProfile::for_model(&request.model);
        let messages = adapt_messages(&request.messages, profile);
        let body = chat_request(&request.model, &messages, self.temperature, self.max_tokens);
        let mut builder = self
            .client
            .post(&self.endpoint)
            .header(header::ACCEPT, "text/event-stream")
            .timeout(self.timeout)
            .json(&body);
        if !self.api_key.is_empty() {
            builder = builder.bearer_auth(&self.api_key);
        }

        let response = tokio::select! {
            _ = cancellation.cancelled() => {
                let _ = events.try_send(ApiEvent::Cancelled { request_id });
                return;
            }
            result = builder.send() => {
                match result {
                    Ok(response) => response,
                    Err(error) => {
                        send_stream_event(
                            &events,
                            ApiEvent::Failed {
                                request_id,
                                message: safe_request_error(&error),
                            },
                            &cancellation,
                        )
                        .await;
                        return;
                    }
                }
            }
        };

        if !response.status().is_success() {
            let message = read_http_error(response, cancellation.clone(), &self.api_key).await;
            if cancellation.is_cancelled() {
                let _ = events.try_send(ApiEvent::Cancelled { request_id });
            } else {
                send_stream_event(
                    &events,
                    ApiEvent::Failed {
                        request_id,
                        message,
                    },
                    &cancellation,
                )
                .await;
            }
            return;
        }

        stream_response(response, request_id, cancellation, events).await;
    }
}

async fn stream_response(
    response: Response,
    request_id: u64,
    cancellation: CancellationToken,
    events: mpsc::Sender<ApiEvent>,
) {
    let mut stream = response.bytes_stream();
    let mut decoder = SseDecoder::default();
    let mut budget = StreamBudget::default();
    let mut saw_valid_event = false;
    let mut saw_text = false;

    loop {
        let item = tokio::select! {
            _ = cancellation.cancelled() => {
                let _ = events.try_send(ApiEvent::Cancelled { request_id });
                return;
            }
            item = stream.next() => item,
        };

        match item {
            Some(Ok(bytes)) => {
                if let Err(error) = budget.record_response_bytes(bytes.len()) {
                    send_stream_event(
                        &events,
                        ApiEvent::Failed {
                            request_id,
                            message: error.to_string(),
                        },
                        &cancellation,
                    )
                    .await;
                    return;
                }
                match decoder.push(&bytes) {
                    Ok(decoded) => {
                        match handle_sse_events(
                            decoded,
                            request_id,
                            &events,
                            &mut saw_valid_event,
                            &mut saw_text,
                            &mut budget,
                            &cancellation,
                        )
                        .await
                        {
                            EventOutcome::Continue => {}
                            EventOutcome::Done => {
                                finish_stream(request_id, saw_text, &events, &cancellation).await;
                                return;
                            }
                            EventOutcome::Failed => return,
                        }
                    }
                    Err(error) => {
                        send_stream_event(
                            &events,
                            ApiEvent::Failed {
                                request_id,
                                message: error.to_string(),
                            },
                            &cancellation,
                        )
                        .await;
                        return;
                    }
                }
            }
            Some(Err(error)) => {
                send_stream_event(
                    &events,
                    ApiEvent::Failed {
                        request_id,
                        message: safe_request_error(&error),
                    },
                    &cancellation,
                )
                .await;
                return;
            }
            None => break,
        }
    }

    match decoder.finish() {
        Ok(decoded) => {
            match handle_sse_events(
                decoded,
                request_id,
                &events,
                &mut saw_valid_event,
                &mut saw_text,
                &mut budget,
                &cancellation,
            )
            .await
            {
                EventOutcome::Continue => {}
                EventOutcome::Done => {
                    finish_stream(request_id, saw_text, &events, &cancellation).await;
                    return;
                }
                EventOutcome::Failed => return,
            }
        }
        Err(error) => {
            send_stream_event(
                &events,
                ApiEvent::Failed {
                    request_id,
                    message: error.to_string(),
                },
                &cancellation,
            )
            .await;
            return;
        }
    }

    if saw_valid_event {
        finish_stream(request_id, saw_text, &events, &cancellation).await;
    } else {
        send_stream_event(
            &events,
            ApiEvent::Failed {
                request_id,
                message: "empty response".to_owned(),
            },
            &cancellation,
        )
        .await;
    }
}

async fn handle_sse_events(
    decoded: Vec<SseEvent>,
    request_id: u64,
    events: &mpsc::Sender<ApiEvent>,
    saw_valid_event: &mut bool,
    saw_text: &mut bool,
    budget: &mut StreamBudget,
    cancellation: &CancellationToken,
) -> EventOutcome {
    let mut delta_batch = String::new();
    let mut latest_usage = None;
    let mut failure = None;
    let mut done = false;

    for event in decoded {
        match event {
            SseEvent::Done => {
                *saw_valid_event = true;
                done = true;
                break;
            }
            SseEvent::Data(data) => match parse_stream_payload(&data) {
                Ok(parsed) => {
                    *saw_valid_event = true;
                    if let Some(content) = parsed.delta {
                        if let Err(error) = budget.record_text_bytes(content.len()) {
                            failure = Some(error.to_string());
                            break;
                        }
                        *saw_text = true;
                        delta_batch.push_str(&content);
                    }
                    if let Some(usage) = parsed.usage {
                        latest_usage = Some(usage);
                    }
                }
                Err(error) => {
                    failure = Some(error.to_string());
                    break;
                }
            },
        }
    }

    if !delta_batch.is_empty()
        && !send_stream_event(
            events,
            ApiEvent::Delta {
                request_id,
                content: delta_batch,
            },
            cancellation,
        )
        .await
    {
        return EventOutcome::Failed;
    }
    if let Some(usage) = latest_usage {
        if !send_stream_event(
            events,
            ApiEvent::Usage {
                request_id,
                prompt_tokens: usage.prompt_tokens,
                completion_tokens: usage.completion_tokens,
                total_tokens: usage.total_tokens,
            },
            cancellation,
        )
        .await
        {
            return EventOutcome::Failed;
        }
    }
    if let Some(message) = failure {
        send_stream_event(
            events,
            ApiEvent::Failed {
                request_id,
                message,
            },
            cancellation,
        )
        .await;
        EventOutcome::Failed
    } else if done {
        EventOutcome::Done
    } else {
        EventOutcome::Continue
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EventOutcome {
    Continue,
    Done,
    Failed,
}

async fn finish_stream(
    request_id: u64,
    saw_text: bool,
    events: &mpsc::Sender<ApiEvent>,
    cancellation: &CancellationToken,
) {
    let event = if saw_text {
        ApiEvent::Finished { request_id }
    } else {
        ApiEvent::Failed {
            request_id,
            message: "empty response".to_owned(),
        }
    };
    send_stream_event(events, event, cancellation).await;
}

async fn send_stream_event(
    events: &mpsc::Sender<ApiEvent>,
    event: ApiEvent,
    cancellation: &CancellationToken,
) -> bool {
    tokio::select! {
        _ = cancellation.cancelled() => false,
        result = events.send(event) => result.is_ok(),
    }
}

async fn read_http_error(
    response: Response,
    cancellation: CancellationToken,
    api_key: &str,
) -> String {
    let status = response.status();
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();

    while body.len() < MAX_ERROR_BODY_BYTES {
        let item = tokio::select! {
            _ = cancellation.cancelled() => return "cancelled".to_owned(),
            item = stream.try_next() => item,
        };
        match item {
            Ok(Some(bytes)) => {
                let remaining = MAX_ERROR_BODY_BYTES - body.len();
                body.extend_from_slice(&bytes[..bytes.len().min(remaining)]);
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }

    let mut detail = extract_api_error_message(&body).unwrap_or_else(|| status_reason(status));
    if !api_key.is_empty() {
        detail = detail.replace(api_key, "[redacted]");
    }
    format!("HTTP {}: {}", status.as_u16(), truncate_message(&detail))
}

#[derive(Deserialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Deserialize)]
struct ErrorBody {
    message: String,
}

pub fn extract_api_error_message(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<ErrorEnvelope>(body)
        .ok()
        .map(|payload| sanitize_text(&payload.error.message))
        .filter(|message| !message.is_empty())
}

fn status_reason(status: StatusCode) -> String {
    status
        .canonical_reason()
        .unwrap_or("request failed")
        .to_owned()
}

fn safe_request_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "request timed out".to_owned()
    } else if error.is_connect() {
        "connection failed".to_owned()
    } else if error.is_decode() {
        "invalid response".to_owned()
    } else if error.is_body() {
        "response body error".to_owned()
    } else {
        "request failed".to_owned()
    }
}

fn sanitize_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_message(value: &str) -> String {
    value.chars().take(MAX_ERROR_MESSAGE_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_json(model: &str) -> serde_json::Value {
        let messages = vec![RequestMessage {
            role: "system",
            content: "Be helpful".into(),
        }];
        let profile = ModelApiProfile::for_model(model);
        let messages = adapt_messages(&messages, profile);
        serde_json::to_value(chat_request(model, &messages, 0.5, 4096))
            .expect("chat request must serialize")
    }

    #[test]
    fn supported_reasoning_models_use_compatible_parameters() {
        for model in &SUPPORTED_OPENAI_MODELS[..8] {
            let request = request_json(model);
            assert_eq!(request["max_completion_tokens"], 4096, "{model}");
            assert!(request.get("max_tokens").is_none(), "{model}");
            assert!(request.get("temperature").is_none(), "{model}");
            assert_eq!(request["messages"][0]["role"], "developer", "{model}");
        }
    }

    #[test]
    fn supported_non_reasoning_models_keep_sampling_parameters() {
        for model in &SUPPORTED_OPENAI_MODELS[8..] {
            let request = request_json(model);
            assert_eq!(request["max_completion_tokens"], 4096, "{model}");
            assert_eq!(request["temperature"], 0.5, "{model}");
            assert!(request.get("max_tokens").is_none(), "{model}");
            assert_eq!(request["messages"][0]["role"], "system", "{model}");
        }
    }

    #[test]
    fn reasoning_snapshots_use_the_reasoning_profile() {
        let request = request_json("gpt-5.4-2026-03-05");
        assert_eq!(request["max_completion_tokens"], 4096);
        assert!(request.get("temperature").is_none());
        assert_eq!(request["messages"][0]["role"], "developer");
    }

    #[test]
    fn unknown_compatible_models_keep_legacy_parameters() {
        let request = request_json("local-chat-model");
        assert_eq!(request["max_tokens"], 4096);
        assert_eq!(request["temperature"], 0.5);
        assert!(request.get("max_completion_tokens").is_none());
        assert_eq!(request["messages"][0]["role"], "system");
    }

    #[test]
    fn parses_sse_event() {
        let mut decoder = SseDecoder::default();
        let events = decoder
            .push(b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n")
            .unwrap_or_default();
        assert_eq!(events.len(), 1);
        let SseEvent::Data(data) = &events[0] else {
            panic!("expected data event");
        };
        assert_eq!(
            parse_stream_payload(data).unwrap_or_default().delta,
            Some("Hello".into())
        );
    }

    #[test]
    fn parses_multiple_events_in_one_chunk() {
        let mut decoder = SseDecoder::default();
        let events = decoder
            .push(b"data: {\"choices\":[]}\n\ndata: [DONE]\r\n\r\n")
            .unwrap_or_default();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1], SseEvent::Done);
    }

    #[test]
    fn buffers_event_split_between_chunks() {
        let mut decoder = SseDecoder::default();
        assert!(decoder.push(b"data: {\"cho").unwrap_or_default().is_empty());
        let events = decoder.push(b"ices\":[]}\n\n").unwrap_or_default();
        assert_eq!(events, vec![SseEvent::Data("{\"choices\":[]}".into())]);
    }

    #[test]
    fn recognizes_done() {
        let mut decoder = SseDecoder::default();
        assert_eq!(
            decoder.push(b"data: [DONE]\n\n").unwrap_or_default(),
            vec![SseEvent::Done]
        );
    }

    #[test]
    fn finds_a_separator_split_across_incremental_scans() {
        let mut decoder = SseDecoder::default();
        assert!(decoder
            .push(b"data: [DONE]\r\n\r")
            .unwrap_or_default()
            .is_empty());
        assert!(decoder.search_offset > 0);
        assert_eq!(
            decoder.push(b"\n").unwrap_or_default(),
            vec![SseEvent::Done]
        );
    }

    #[test]
    fn empty_sse_blocks_cannot_bypass_the_stream_event_limit() {
        let mut decoder = SseDecoder {
            blocks_seen: MAX_SSE_BLOCKS,
            ..SseDecoder::default()
        };
        assert!(matches!(
            decoder.push(b"\n\n"),
            Err(StreamError::TooManyBlocks)
        ));
    }

    #[test]
    fn ignores_empty_delta() {
        let parsed =
            parse_stream_payload(r#"{"choices":[{"delta":{"content":""}}]}"#).unwrap_or_default();
        assert_eq!(parsed.delta, None);
    }

    #[test]
    fn extracts_api_error() {
        assert_eq!(
            extract_api_error_message(
                br#"{"error":{"message":"Invalid API key","type":"invalid_request_error"}}"#
            ),
            Some("Invalid API key".into())
        );
        assert_eq!(extract_api_error_message(b"<html>no</html>"), None);
    }

    #[test]
    fn supports_multiline_data_and_comments() {
        let mut decoder = SseDecoder::default();
        let events = decoder
            .push(b": keepalive\ndata: first\ndata: second\n\n")
            .unwrap_or_default();
        assert_eq!(events, vec![SseEvent::Data("first\nsecond".into())]);
    }

    #[test]
    fn rejects_an_oversized_unterminated_event() {
        let mut decoder = SseDecoder::default();
        let oversized = vec![b'x'; MAX_SSE_EVENT_BYTES + 1];
        assert!(matches!(
            decoder.push(&oversized),
            Err(StreamError::EventTooLarge)
        ));
    }

    #[test]
    fn rejects_an_oversized_terminated_event() {
        let mut decoder = SseDecoder::default();
        let mut oversized = vec![b'x'; MAX_SSE_EVENT_BYTES + 1];
        oversized.extend_from_slice(b"\n\n");
        assert!(matches!(
            decoder.push(&oversized),
            Err(StreamError::EventTooLarge)
        ));
    }

    #[test]
    fn accepts_multiple_bounded_events_in_a_large_chunk() {
        let padding = "x".repeat(MAX_SSE_EVENT_BYTES / 2);
        let event = format!(": {padding}\ndata: [DONE]\n\n");
        let chunk = event.repeat(3);
        assert!(chunk.len() > MAX_SSE_EVENT_BYTES);

        let events = decoder_events(chunk.as_bytes());
        assert_eq!(events.len(), 3);
        assert!(events.iter().all(|event| *event == SseEvent::Done));
    }

    #[test]
    fn stream_budget_enforces_all_limits_and_overflow() {
        let mut budget = StreamBudget::default();
        assert!(budget
            .record_response_bytes(MAX_STREAM_RESPONSE_BYTES)
            .is_ok());
        assert!(matches!(
            budget.record_response_bytes(1),
            Err(StreamError::ResponseTooLarge)
        ));

        let mut budget = StreamBudget::default();
        assert!(budget.record_text_bytes(MAX_STREAM_TEXT_BYTES).is_ok());
        assert!(matches!(
            budget.record_text_bytes(1),
            Err(StreamError::TextTooLarge)
        ));

        assert!(matches!(
            checked_total(usize::MAX, 1, usize::MAX, StreamError::ResponseTooLarge),
            Err(StreamError::ResponseTooLarge)
        ));
    }

    fn decoder_events(chunk: &[u8]) -> Vec<SseEvent> {
        SseDecoder::default().push(chunk).unwrap_or_default()
    }

    #[tokio::test]
    async fn reports_empty_successful_stream_as_error() {
        let (sender, mut receiver) = mpsc::channel(1);
        finish_stream(9, false, &sender, &CancellationToken::new()).await;
        let event = receiver.recv().await;
        assert!(matches!(
            event,
            Some(ApiEvent::Failed {
                request_id: 9,
                message
            }) if message == "empty response"
        ));
    }

    #[tokio::test]
    async fn batches_deltas_before_crossing_the_bounded_channel() {
        let decoded = vec![
            SseEvent::Data(r#"{"choices":[{"delta":{"content":"Hello"}}]}"#.into()),
            SseEvent::Data(r#"{"choices":[{"delta":{"content":" world"}}]}"#.into()),
        ];
        let (sender, mut receiver) = mpsc::channel(1);
        let mut saw_valid_event = false;
        let mut saw_text = false;
        let outcome = handle_sse_events(
            decoded,
            7,
            &sender,
            &mut saw_valid_event,
            &mut saw_text,
            &mut StreamBudget::default(),
            &CancellationToken::new(),
        )
        .await;

        assert_eq!(outcome, EventOutcome::Continue);
        assert!(saw_valid_event);
        assert!(saw_text);
        assert!(matches!(
            receiver.recv().await,
            Some(ApiEvent::Delta {
                request_id: 7,
                content
            }) if content == "Hello world"
        ));
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn cancellation_unblocks_a_full_event_channel() {
        let (sender, _receiver) = mpsc::channel(1);
        sender
            .send(ApiEvent::Started { request_id: 1 })
            .await
            .expect("test receiver must stay open");
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        assert!(
            !send_stream_event(&sender, ApiEvent::Finished { request_id: 1 }, &cancellation).await
        );
    }
}
