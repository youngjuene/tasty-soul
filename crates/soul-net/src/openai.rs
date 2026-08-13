//! OpenAI 호환 클라이언트 (§9.8 · §10 · §15).
//!
//! - 이미지·텍스트: **Responses API**
//! - 오디오: **Chat Completions의 `input_audio`** (Responses API는 오디오를 받지 않는다, §9.5)
//! - 전부 structured output (JSON schema, strict) — §10
//!
//! ## 오류 처리 (§15)
//!
//! | 상황 | 동작 |
//! |---|---|
//! | 429 / 5xx | 지수 백오프 + jitter, **최대 3회** 재시도. 호출당 상한은 `timeout_secs` |
//! | 4xx | **재시도 없음.** 트레이스 기록 후 사용자에게 표시 |
//! | 하드 실패 | **관측을 만들지 않는다.** 원본은 캐시에 남기고 재시도 버튼 제공 |
//!
//! ## 이 파일의 두 가지 구조적 약속
//!
//! 1. **요청 body 조립은 전부 순수 함수다** (`build_*_body`). 네트워크 없이 JSON 모양을
//!    테스트할 수 있어야 §10의 계약이 조용히 깨지지 않는다.
//! 2. **모든 실패 메시지는 응답 원문 앞 500자를 담는다.** 응답 형태가 바뀌었을 때
//!    "파싱 실패"만 남으면 디버깅이 불가능하다.

use base64::Engine;
use serde_json::{json, Value};
use soul_core::config::Config;
use soul_core::error::{Result, SoulError};
use std::time::{Duration, Instant};

/// §15 — 429/5xx 재시도 상한. **최초 시도는 여기에 포함되지 않는다** (총 요청 수는 최대 4회).
pub const MAX_RETRIES: u32 = 3;

/// 백오프 기본 간격. 500ms → 1s → 2s.
const BACKOFF_BASE_MS: u64 = 500;
/// `Retry-After`가 터무니없이 크게 와도 여기서 잘린다.
const BACKOFF_CAP_MS: u64 = 20_000;
/// 오류 메시지에 담는 응답 원문 길이.
const RAW_SNIPPET_CHARS: usize = 500;

pub struct OpenAi {
    pub base_url: String,
    pub timeout_secs: u64,
    client: reqwest::Client,
    api_key: String,
}

/// 한 번의 API 호출 결과. 트레이스(§11.3)에 넣을 값을 함께 돌려준다.
pub struct Call<T> {
    pub value: T,
    pub call_id: String,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub latency_ms: u64,
    pub raw: String,
}

/// Responses API 입력 파트.
pub enum Part {
    Text(String),
    /// base64 data URL 또는 원격 URL. **YouTube 썸네일은 URL 그대로 넘긴다** (§9.3) —
    /// 사용자 IP가 YouTube 썸네일 서버에 닿지 않는다.
    ImageUrl(String),
}

impl OpenAi {
    pub fn new(config: &Config, api_key: String) -> Result<OpenAi> {
        let key = api_key.trim().to_string();
        if key.is_empty() {
            // §15 "키 미설정" — 여기서 막지 않으면 Authorization 헤더가 빈 채로 나가
            // 401을 4xx 하드 실패로 보고하게 된다. 원인이 한 겹 멀어진다.
            return Err(SoulError::config(
                "OpenAI API 키가 설정되지 않았습니다. 설정 화면에서 키를 입력하세요",
            ));
        }
        let base_url = config.api.base_url.trim().trim_end_matches('/').to_string();
        if base_url.is_empty() {
            return Err(SoulError::config("api.base_url 이 비어 있습니다"));
        }
        // 0이면 "무제한"이 되어 §15의 호출당 상한 약속이 사라진다.
        let timeout_secs = config.api.timeout_secs.max(1);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| SoulError::config(format!("HTTP 클라이언트를 만들 수 없습니다: {e}")))?;
        Ok(OpenAi {
            base_url,
            timeout_secs,
            client,
            api_key: key,
        })
    }

    /// Responses API, structured output. `schema`는 strict JSON Schema.
    pub async fn responses_json(
        &self,
        model: &str,
        system: &str,
        parts: Vec<Part>,
        schema_name: &str,
        schema: &serde_json::Value,
    ) -> Result<Call<serde_json::Value>> {
        let body = build_responses_body(model, system, &parts, schema_name, schema);
        let (raw, latency_ms) = self
            .send(reqwest::Method::POST, "responses", Some(&body))
            .await?;
        let v = parse_body(&raw)?;
        let text = match extract_output_text(&v) {
            OutputText::Text(t) => t,
            OutputText::Refusal(r) => return Err(refusal_error(&r)),
            OutputText::Missing => {
                return Err(unexpected_shape("output_text 를 찾지 못했습니다", &raw))
            }
        };
        Ok(finish_call(parse_payload(&text)?, &v, latency_ms, raw))
    }

    /// Chat Completions `input_audio`, structured output (§9.5).
    pub async fn audio_json(
        &self,
        model: &str,
        system: &str,
        text_parts: Vec<String>,
        audio_mp3: &[u8],
        schema_name: &str,
        schema: &serde_json::Value,
    ) -> Result<Call<serde_json::Value>> {
        let b64 = base64::engine::general_purpose::STANDARD.encode(audio_mp3);
        let body = build_audio_body(model, system, &text_parts, &b64, schema_name, schema);
        let (raw, latency_ms) = self
            .send(reqwest::Method::POST, "chat/completions", Some(&body))
            .await?;
        let v = parse_body(&raw)?;
        let msg = extract_chat_message(&v)
            .ok_or_else(|| unexpected_shape("choices[0].message 가 없습니다", &raw))?;
        let text = match extract_chat_content(msg) {
            OutputText::Text(t) => t,
            OutputText::Refusal(r) => return Err(refusal_error(&r)),
            OutputText::Missing => {
                return Err(unexpected_shape(
                    "choices[0].message.content 가 비어 있습니다",
                    &raw,
                ))
            }
        };
        Ok(finish_call(parse_payload(&text)?, &v, latency_ms, raw))
    }

    /// function calling 루프 한 턴 (§11). 툴 정의와 대화 이력을 받아 다음 수를 돌려준다.
    ///
    /// 반환값은 **assistant 메시지 전체**다 (`tool_calls` 포함). 호출자는 이것을 그대로
    /// `messages`에 이어 붙이고 툴 결과를 덧붙여 다음 턴을 돈다.
    pub async fn tool_turn(
        &self,
        model: &str,
        system: &str,
        messages: &[serde_json::Value],
        tools: &[serde_json::Value],
    ) -> Result<Call<serde_json::Value>> {
        let body = build_tool_body(model, system, messages, tools);
        let (raw, latency_ms) = self
            .send(reqwest::Method::POST, "chat/completions", Some(&body))
            .await?;
        let v = parse_body(&raw)?;
        let msg = extract_chat_message(&v)
            .ok_or_else(|| unexpected_shape("choices[0].message 가 없습니다", &raw))?
            .clone();
        Ok(finish_call(msg, &v, latency_ms, raw))
    }

    /// `GET {base_url}/models` (§9.9 단계 1).
    pub async fn list_models(&self) -> Result<Vec<String>> {
        let (raw, _) = self.send(reqwest::Method::GET, "models", None).await?;
        let v = parse_body(&raw)?;
        parse_models(&v).map_err(|why| unexpected_shape(&why, &raw))
    }

    /// 임베딩. `dimensions` 파라미터를 반드시 보낸다 (§20.2 Matryoshka 절단).
    pub async fn embed(&self, model: &str, dims: usize, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let body = build_embed_body(model, dims, texts);
        let (raw, _) = self
            .send(reqwest::Method::POST, "embeddings", Some(&body))
            .await?;
        let v = parse_body(&raw)?;
        parse_embeddings(&v, dims, texts.len()).map_err(|why| unexpected_shape(&why, &raw))
    }

    /// §15의 재시도 정책을 담은 유일한 전송 지점. 반환은 `(응답 원문, 총 소요 ms)`.
    ///
    /// `latency_ms`는 재시도 대기까지 포함한 **사용자가 실제로 기다린 시간**이다.
    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(String, u64)> {
        let url = self.url(path);
        let started = Instant::now();
        let mut attempt: u32 = 0;
        loop {
            let mut req = self
                .client
                .request(method.clone(), &url)
                .bearer_auth(&self.api_key);
            if let Some(b) = body {
                req = req.json(b);
            }
            let delay_ms = match req.send().await {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    // headers()는 text()가 resp를 소비하기 전에 읽어야 한다.
                    let retry_after = resp
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(parse_retry_after);
                    let text = resp.text().await.unwrap_or_default();
                    if (200..300).contains(&status) {
                        return Ok((text, started.elapsed().as_millis() as u64));
                    }
                    if !is_retryable_status(status) || attempt >= MAX_RETRIES {
                        // 4xx는 여기로 온다 — 재시도 없이 바로 사용자에게 (§15).
                        return Err(api_error(status, &text, attempt));
                    }
                    // 서버가 대기 시간을 알려주면 그 말을 듣는다. 429에서 우리 백오프가
                    // 더 짧으면 곧바로 다시 맞는다.
                    retry_after.unwrap_or_else(|| backoff_delay_ms(attempt, jitter_seed()))
                }
                Err(e) => {
                    // 전송 계층 실패(타임아웃·연결 끊김)는 5xx와 성격이 같으므로 같이 재시도한다.
                    if attempt >= MAX_RETRIES {
                        return Err(SoulError::invalid(format!(
                            "OpenAI 요청 실패 ({url}, {}회 재시도 후): {e}",
                            attempt
                        )));
                    }
                    backoff_delay_ms(attempt, jitter_seed())
                }
            };
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            attempt += 1;
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path.trim_start_matches('/'))
    }
}

// ── 요청 body 조립 (순수 함수) ────────────────────────────────────────────────

fn part_json(p: &Part) -> Value {
    match p {
        Part::Text(t) => json!({ "type": "input_text", "text": t }),
        // data URL이든 http URL이든 같은 필드에 넣는다 (§9.3·§9.4).
        Part::ImageUrl(u) => json!({ "type": "input_image", "image_url": u }),
    }
}

/// Responses API 요청 body (§10).
fn build_responses_body(
    model: &str,
    system: &str,
    parts: &[Part],
    schema_name: &str,
    schema: &Value,
) -> Value {
    let content: Vec<Value> = parts.iter().map(part_json).collect();
    json!({
        "model": model,
        "instructions": system,
        "input": [{ "role": "user", "content": content }],
        "text": {
            "format": {
                "type": "json_schema",
                "name": schema_name,
                "schema": schema,
                "strict": true
            }
        }
    })
}

/// Chat Completions `input_audio` 요청 body (§9.5).
/// `audio_b64`는 이미 base64로 인코딩된 mp3다.
fn build_audio_body(
    model: &str,
    system: &str,
    text_parts: &[String],
    audio_b64: &str,
    schema_name: &str,
    schema: &Value,
) -> Value {
    let mut content: Vec<Value> = text_parts
        .iter()
        .map(|t| json!({ "type": "text", "text": t }))
        .collect();
    content.push(json!({
        "type": "input_audio",
        "input_audio": { "data": audio_b64, "format": "mp3" }
    }));
    json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": content }
        ],
        "response_format": {
            "type": "json_schema",
            "json_schema": { "name": schema_name, "schema": schema, "strict": true }
        }
    })
}

/// function calling 한 턴의 요청 body (§11).
/// **`tools`가 비면 `tools` 키 자체를 넣지 않는다** — 빈 배열은 API가 거부한다.
fn build_tool_body(model: &str, system: &str, messages: &[Value], tools: &[Value]) -> Value {
    let mut msgs = Vec::with_capacity(messages.len() + 1);
    msgs.push(json!({ "role": "system", "content": system }));
    msgs.extend(messages.iter().cloned());
    let mut body = json!({ "model": model, "messages": msgs });
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools.to_vec());
        body["tool_choice"] = json!("auto");
    }
    body
}

/// 임베딩 요청 body. **`dimensions`가 빠지면 §20.2가 통째로 무너진다** —
/// 1536차원이 돌아와 캐시 키(dims=256)와 벡터 길이가 어긋난다.
fn build_embed_body(model: &str, dims: usize, texts: &[String]) -> Value {
    json!({ "model": model, "input": texts, "dimensions": dims })
}

// ── 응답 해석 (순수 함수) ─────────────────────────────────────────────────────

/// 모델이 돌려준 본문 텍스트의 세 가지 결말.
#[derive(Debug, PartialEq)]
enum OutputText {
    Text(String),
    /// 안전 거부. 파싱 실패와 구분해야 사용자에게 다른 안내를 할 수 있다.
    Refusal(String),
    Missing,
}

/// Responses API 응답에서 본문 텍스트를 뽑는다.
/// `output_text`(문자열 또는 문자열 배열)를 먼저 보고, 없으면 `output[].content[].text`를 잇는다.
fn extract_output_text(v: &Value) -> OutputText {
    match &v["output_text"] {
        Value::String(s) if !s.is_empty() => return OutputText::Text(s.clone()),
        Value::Array(items) => {
            let joined: String = items.iter().filter_map(|i| i.as_str()).collect();
            if !joined.is_empty() {
                return OutputText::Text(joined);
            }
        }
        _ => {}
    }

    let mut buf = String::new();
    if let Some(items) = v["output"].as_array() {
        for item in items {
            let parts = match item["content"].as_array() {
                Some(p) => p,
                // `{"type":"output_text","text":...}`가 output 바로 아래 오는 형태도 받는다.
                None => std::slice::from_ref(item),
            };
            for part in parts {
                if part["type"] == "refusal" {
                    if let Some(r) = part["refusal"].as_str() {
                        return OutputText::Refusal(r.to_string());
                    }
                }
                if let Some(t) = part["text"].as_str() {
                    buf.push_str(t);
                }
            }
        }
    }
    if buf.is_empty() {
        OutputText::Missing
    } else {
        OutputText::Text(buf)
    }
}

/// Chat Completions 응답의 assistant 메시지.
fn extract_chat_message(v: &Value) -> Option<&Value> {
    let msg = v["choices"].as_array()?.first()?.get("message")?;
    msg.is_object().then_some(msg)
}

/// assistant 메시지의 본문. `content`가 문자열이 아니라 파트 배열인 경우도 받는다.
fn extract_chat_content(msg: &Value) -> OutputText {
    if let Some(r) = msg["refusal"].as_str() {
        if !r.is_empty() {
            return OutputText::Refusal(r.to_string());
        }
    }
    match &msg["content"] {
        Value::String(s) if !s.trim().is_empty() => OutputText::Text(s.clone()),
        Value::Array(parts) => {
            let joined: String = parts.iter().filter_map(|p| p["text"].as_str()).collect();
            if joined.trim().is_empty() {
                OutputText::Missing
            } else {
                OutputText::Text(joined)
            }
        }
        _ => OutputText::Missing,
    }
}

/// `usage`에서 토큰 수. Responses(`input_tokens`)와 Chat(`prompt_tokens`) 양쪽 이름을 본다.
fn usage_tokens(v: &Value) -> (u64, u64) {
    let u = &v["usage"];
    let tin = u["input_tokens"]
        .as_u64()
        .or_else(|| u["prompt_tokens"].as_u64())
        .unwrap_or(0);
    let tout = u["output_tokens"]
        .as_u64()
        .or_else(|| u["completion_tokens"].as_u64())
        .unwrap_or(0);
    (tin, tout)
}

/// `GET /models` → `data[].id` 오름차순.
fn parse_models(v: &Value) -> std::result::Result<Vec<String>, String> {
    let data = v["data"]
        .as_array()
        .ok_or_else(|| "data 배열이 없습니다".to_string())?;
    let mut ids: Vec<String> = data
        .iter()
        .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
        .collect();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// `POST /embeddings` → 입력 순서대로 정렬된 벡터. `index`가 있으면 그것을 따른다.
fn parse_embeddings(
    v: &Value,
    dims: usize,
    n: usize,
) -> std::result::Result<Vec<Vec<f32>>, String> {
    let data = v["data"]
        .as_array()
        .ok_or_else(|| "data 배열이 없습니다".to_string())?;
    if data.len() != n {
        return Err(format!(
            "임베딩 개수 불일치: {}개 요청, {}개 응답",
            n,
            data.len()
        ));
    }
    let mut out: Vec<Option<Vec<f32>>> = vec![None; n];
    for (pos, item) in data.iter().enumerate() {
        let idx = item["index"].as_u64().map(|i| i as usize).unwrap_or(pos);
        if idx >= n {
            return Err(format!("임베딩 index 가 범위를 벗어났습니다: {idx}"));
        }
        let arr = item["embedding"]
            .as_array()
            .ok_or_else(|| format!("data[{pos}].embedding 이 배열이 아닙니다"))?;
        if arr.len() != dims {
            // §20.2 — `dimensions`를 무시하는 모델은 여기서 걸러야 한다.
            // 그냥 받으면 캐시 키(dims)와 벡터 길이가 영구히 어긋난다.
            return Err(format!(
                "임베딩 차원 불일치: dims={dims} 를 요청했는데 {}차원이 왔습니다 \
                 (모델이 dimensions 파라미터를 지원하지 않습니다)",
                arr.len()
            ));
        }
        let vec: Vec<f32> = arr
            .iter()
            .map(|x| x.as_f64().unwrap_or(0.0) as f32)
            .collect();
        out[idx] = Some(vec);
    }
    out.into_iter()
        .enumerate()
        .map(|(i, v)| v.ok_or_else(|| format!("임베딩 {i}번이 비었습니다")))
        .collect()
}

fn call_id(v: &Value) -> String {
    v["id"].as_str().unwrap_or_default().to_string()
}

fn finish_call(value: Value, resp: &Value, latency_ms: u64, raw: String) -> Call<Value> {
    let (tokens_in, tokens_out) = usage_tokens(resp);
    Call {
        value,
        call_id: call_id(resp),
        tokens_in,
        tokens_out,
        latency_ms,
        raw,
    }
}

fn parse_body(raw: &str) -> Result<Value> {
    serde_json::from_str(raw)
        .map_err(|e| unexpected_shape(&format!("응답이 JSON이 아닙니다: {e}"), raw))
}

/// structured output 본문을 JSON으로 읽는다 (§10).
fn parse_payload(text: &str) -> Result<Value> {
    serde_json::from_str(text.trim()).map_err(|e| {
        SoulError::invalid(format!(
            "structured output 파싱 실패: {e}. 모델 출력 앞 {RAW_SNIPPET_CHARS}자: {}",
            snippet(text)
        ))
    })
}

// ── 오류 메시지 ──────────────────────────────────────────────────────────────

/// 앞 `RAW_SNIPPET_CHARS`자. **문자 경계로 자른다** — 한국어 응답에서 바이트로 자르면 패닉이다.
fn snippet(s: &str) -> String {
    match s.char_indices().nth(RAW_SNIPPET_CHARS) {
        None => s.to_string(),
        Some((idx, _)) => format!("{}…(생략)", &s[..idx]),
    }
}

fn api_error(status: u16, raw: &str, retries: u32) -> SoulError {
    let tail = if retries > 0 {
        format!(" ({retries}회 재시도 후)")
    } else {
        String::new()
    };
    SoulError::invalid(format!(
        "OpenAI API 오류 {status}{tail}. 원문 앞 {RAW_SNIPPET_CHARS}자: {}",
        snippet(raw)
    ))
}

fn unexpected_shape(what: &str, raw: &str) -> SoulError {
    SoulError::invalid(format!(
        "OpenAI 응답 형태가 예상과 다릅니다 — {what}. 원문 앞 {RAW_SNIPPET_CHARS}자: {}",
        snippet(raw)
    ))
}

fn refusal_error(reason: &str) -> SoulError {
    SoulError::invalid(format!("모델이 응답을 거부했습니다: {}", snippet(reason)))
}

// ── 재시도 (§15) ─────────────────────────────────────────────────────────────

/// 429와 5xx만 재시도한다. **4xx는 재시도하지 않는다** — 같은 요청은 같은 이유로 실패한다.
fn is_retryable_status(status: u16) -> bool {
    status == 429 || (500..600).contains(&status)
}

/// `Retry-After: 12` (초). HTTP-date 형식은 해석하지 않고 백오프로 넘긴다.
fn parse_retry_after(v: &str) -> Option<u64> {
    let secs: u64 = v.trim().parse().ok()?;
    Some(secs.saturating_mul(1000).min(BACKOFF_CAP_MS))
}

/// jitter용 난수 씨앗. **결정론(§R5)과 무관한 자리**이므로 시간 기반 해시로 만든다
/// — 이것 하나 때문에 `rand` 크레이트를 넣지 않는다.
fn jitter_seed() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    splitmix64(nanos)
}

fn splitmix64(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// `attempt`(0-기준)번째 재시도 직전 대기 시간(ms).
///
/// 기본 500ms에서 2배씩 늘리고 그 절반 범위의 jitter를 더한다.
/// jitter가 없으면 여러 작업(§9.10의 `critique_concurrency`)이 같은 순간에 몰려
/// 429를 다시 맞는다.
fn backoff_delay_ms(attempt: u32, seed: u64) -> u64 {
    let base = BACKOFF_BASE_MS
        .saturating_mul(1u64 << attempt.min(16))
        .min(BACKOFF_CAP_MS);
    let jitter = splitmix64(seed ^ u64::from(attempt)) % (base / 2 + 1);
    base.saturating_add(jitter).min(BACKOFF_CAP_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Value {
        json!({ "type": "object", "additionalProperties": false, "required": ["prose"] })
    }

    // ── 클라이언트 생성 ─────────────────────────────────────────────────

    #[test]
    fn new_trims_trailing_slash_and_keeps_timeout() {
        let mut cfg = Config::default();
        cfg.api.base_url = "https://example.test/v1/".into();
        cfg.api.timeout_secs = 42;
        let c = OpenAi::new(&cfg, "sk-test".into()).unwrap();
        assert_eq!(c.base_url, "https://example.test/v1");
        assert_eq!(c.timeout_secs, 42);
        assert_eq!(c.url("responses"), "https://example.test/v1/responses");
        assert_eq!(c.url("/models"), "https://example.test/v1/models");
    }

    #[test]
    fn new_rejects_missing_key() {
        // §15 "키 미설정" — 401을 기다리지 않고 여기서 막는다.
        let cfg = Config::default();
        // `OpenAi`는 Debug를 구현하지 않는다 — api_key가 로그로 새면 안 되므로
        // `unwrap_err()` 대신 `err()`로 꺼낸다.
        let err = OpenAi::new(&cfg, "  ".into()).err().unwrap();
        assert!(matches!(err, SoulError::Config(_)), "받은 에러: {err}");
    }

    #[test]
    fn new_clamps_zero_timeout() {
        let mut cfg = Config::default();
        cfg.api.timeout_secs = 0;
        assert_eq!(OpenAi::new(&cfg, "k".into()).unwrap().timeout_secs, 1);
    }

    // ── 요청 body (§10) ─────────────────────────────────────────────────

    #[test]
    fn responses_body_matches_spec() {
        let parts = vec![
            Part::Text("무엇이 보이는가".into()),
            Part::ImageUrl("data:image/jpeg;base64,AAAA".into()),
        ];
        let b = build_responses_body("m-vision", "너는 관찰자다", &parts, "describe", &schema());

        assert_eq!(b["model"], "m-vision");
        assert_eq!(b["instructions"], "너는 관찰자다");
        assert_eq!(b["input"][0]["role"], "user");
        assert_eq!(b["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(b["input"][0]["content"][0]["text"], "무엇이 보이는가");
        assert_eq!(b["input"][0]["content"][1]["type"], "input_image");
        assert_eq!(
            b["input"][0]["content"][1]["image_url"],
            "data:image/jpeg;base64,AAAA"
        );
        // strict structured output — §10의 전제다.
        assert_eq!(b["text"]["format"]["type"], "json_schema");
        assert_eq!(b["text"]["format"]["name"], "describe");
        assert_eq!(b["text"]["format"]["strict"], true);
        assert_eq!(b["text"]["format"]["schema"], schema());
    }

    #[test]
    fn responses_body_keeps_frame_order_for_video() {
        // §9.6 — 프레임 전체를 한 호출에 배열로 넣는다. 순서가 곧 시간축이다.
        let parts: Vec<Part> = (0..30)
            .map(|i| Part::ImageUrl(format!("data:image/jpeg;base64,{i}")))
            .collect();
        let b = build_responses_body("m", "s", &parts, "n", &schema());
        let content = b["input"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 30);
        assert_eq!(content[0]["image_url"], "data:image/jpeg;base64,0");
        assert_eq!(content[29]["image_url"], "data:image/jpeg;base64,29");
    }

    #[test]
    fn audio_body_uses_chat_completions_shape() {
        let b = build_audio_body(
            "m-audio",
            "너는 청취자다",
            &["제목: 어떤 곡".to_string()],
            "QUJD",
            "describe",
            &schema(),
        );
        assert_eq!(b["messages"][0]["role"], "system");
        assert_eq!(b["messages"][0]["content"], "너는 청취자다");
        assert_eq!(b["messages"][1]["role"], "user");
        assert_eq!(b["messages"][1]["content"][0]["type"], "text");
        assert_eq!(b["messages"][1]["content"][0]["text"], "제목: 어떤 곡");
        assert_eq!(b["messages"][1]["content"][1]["type"], "input_audio");
        assert_eq!(
            b["messages"][1]["content"][1]["input_audio"]["data"],
            "QUJD"
        );
        assert_eq!(
            b["messages"][1]["content"][1]["input_audio"]["format"],
            "mp3"
        );
        // Chat Completions는 `response_format` 아래 한 겹 더 들어간다.
        assert_eq!(b["response_format"]["type"], "json_schema");
        assert_eq!(b["response_format"]["json_schema"]["name"], "describe");
        assert_eq!(b["response_format"]["json_schema"]["strict"], true);
    }

    #[test]
    fn audio_encodes_mp3_as_base64() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"ABC");
        assert_eq!(b64, "QUJD");
    }

    #[test]
    fn tool_body_prepends_system_and_sets_auto() {
        let msgs = vec![
            json!({"role":"user","content":"시작"}),
            json!({"role":"assistant","tool_calls":[{"id":"c1"}]}),
            json!({"role":"tool","tool_call_id":"c1","content":"[]"}),
        ];
        let tools = vec![json!({"type":"function","function":{"name":"read_soul"}})];
        let b = build_tool_body("m-reflect", "가드레일", &msgs, &tools);

        assert_eq!(b["messages"][0]["role"], "system");
        assert_eq!(b["messages"][0]["content"], "가드레일");
        // 이력은 순서와 내용을 그대로 보존한다 — 루프가 여기 의존한다.
        assert_eq!(b["messages"].as_array().unwrap().len(), 4);
        assert_eq!(b["messages"][2]["tool_calls"][0]["id"], "c1");
        assert_eq!(b["messages"][3]["role"], "tool");
        assert_eq!(b["tool_choice"], "auto");
        assert_eq!(b["tools"][0]["function"]["name"], "read_soul");
    }

    #[test]
    fn tool_body_omits_empty_tools() {
        let b = build_tool_body("m", "s", &[], &[]);
        assert!(b.get("tools").is_none(), "빈 tools 배열은 API가 거부한다");
        assert!(b.get("tool_choice").is_none());
    }

    #[test]
    fn embed_body_always_sends_dimensions() {
        // §20.2 — 이 키가 빠지면 1536차원이 돌아와 캐시 키가 거짓말을 한다.
        let b = build_embed_body(
            "text-embedding-3-small",
            256,
            &["차갑고 정돈된 실내".into()],
        );
        assert_eq!(b["dimensions"], 256);
        assert_eq!(b["model"], "text-embedding-3-small");
        assert_eq!(b["input"][0], "차갑고 정돈된 실내");
    }

    // ── 응답 해석 ───────────────────────────────────────────────────────

    #[test]
    fn output_text_from_convenience_field() {
        let v = json!({ "output_text": "{\"prose\":\"ok\"}" });
        assert_eq!(
            extract_output_text(&v),
            OutputText::Text("{\"prose\":\"ok\"}".into())
        );
    }

    #[test]
    fn output_text_from_output_array() {
        let v = json!({
            "id": "resp_1",
            "output": [{
                "type": "message",
                "content": [{ "type": "output_text", "text": "{\"prose\":\"차분한 실내\"}" }]
            }]
        });
        assert_eq!(
            extract_output_text(&v),
            OutputText::Text("{\"prose\":\"차분한 실내\"}".into())
        );
    }

    #[test]
    fn output_text_joins_multiple_chunks() {
        let v = json!({
            "output": [
                { "content": [{ "type": "output_text", "text": "{\"a\":" }] },
                { "content": [{ "type": "output_text", "text": "1}" }] }
            ]
        });
        assert_eq!(
            extract_output_text(&v),
            OutputText::Text("{\"a\":1}".into())
        );
    }

    #[test]
    fn refusal_is_distinguished_from_missing() {
        let v = json!({
            "output": [{ "content": [{ "type": "refusal", "refusal": "그건 못 하겠습니다" }] }]
        });
        assert_eq!(
            extract_output_text(&v),
            OutputText::Refusal("그건 못 하겠습니다".into())
        );
        assert_eq!(extract_output_text(&json!({"id":"x"})), OutputText::Missing);
    }

    #[test]
    fn chat_message_keeps_tool_calls() {
        // tool_turn 이 assistant 메시지를 통째로 돌려줘야 루프가 이어진다 (§11).
        let v = json!({
            "id": "chatcmpl_1",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "list_observations", "arguments": "{\"limit\":50}" }
                    }]
                }
            }]
        });
        let msg = extract_chat_message(&v).unwrap();
        assert_eq!(
            msg["tool_calls"][0]["function"]["name"],
            "list_observations"
        );
        assert_eq!(extract_chat_content(msg), OutputText::Missing);
        assert!(extract_chat_message(&json!({"choices":[]})).is_none());
    }

    #[test]
    fn chat_content_reads_string_and_parts_and_refusal() {
        assert_eq!(
            extract_chat_content(&json!({"content":"{\"x\":1}"})),
            OutputText::Text("{\"x\":1}".into())
        );
        assert_eq!(
            extract_chat_content(&json!({"content":[{"type":"text","text":"본문"}]})),
            OutputText::Text("본문".into())
        );
        assert_eq!(
            extract_chat_content(&json!({"content":null,"refusal":"거부"})),
            OutputText::Refusal("거부".into())
        );
    }

    #[test]
    fn usage_reads_both_naming_schemes() {
        assert_eq!(
            usage_tokens(&json!({"usage":{"input_tokens":11,"output_tokens":22}})),
            (11, 22)
        );
        assert_eq!(
            usage_tokens(&json!({"usage":{"prompt_tokens":3,"completion_tokens":4}})),
            (3, 4)
        );
        assert_eq!(usage_tokens(&json!({})), (0, 0));
    }

    #[test]
    fn models_are_sorted_and_deduped() {
        let v = json!({"data":[{"id":"gpt-z"},{"id":"gpt-a"},{"id":"gpt-a"},{"no_id":1}]});
        assert_eq!(parse_models(&v).unwrap(), vec!["gpt-a", "gpt-z"]);
        assert!(parse_models(&json!({"error":"nope"})).is_err());
    }

    #[test]
    fn embeddings_are_returned_in_input_order() {
        let v = json!({"data":[
            {"index":1,"embedding":[0.5,0.25]},
            {"index":0,"embedding":[1.0,0.0]}
        ]});
        assert_eq!(
            parse_embeddings(&v, 2, 2).unwrap(),
            vec![vec![1.0, 0.0], vec![0.5, 0.25]]
        );
    }

    #[test]
    fn embeddings_reject_wrong_dimension() {
        // §20.2 — dimensions 를 무시하는 모델은 doctor 단계에서 실패로 처리해야 한다 (§9.9).
        let v = json!({"data":[{"index":0,"embedding":[1.0,0.0,0.0]}]});
        let err = parse_embeddings(&v, 256, 1).unwrap_err();
        assert!(err.contains("차원 불일치"), "받은 메시지: {err}");
        let v = json!({"data":[]});
        assert!(parse_embeddings(&v, 2, 1).is_err());
    }

    #[test]
    fn payload_parse_failure_carries_the_raw_text() {
        let err = parse_payload("이건 JSON이 아니다").unwrap_err().to_string();
        assert!(err.contains("이건 JSON이 아니다"), "받은 메시지: {err}");
    }

    // ── 오류 메시지 ─────────────────────────────────────────────────────

    #[test]
    fn snippet_cuts_on_char_boundary() {
        // 한국어 3바이트 문자를 바이트로 자르면 패닉이다.
        let long = "가".repeat(RAW_SNIPPET_CHARS + 50);
        let s = snippet(&long);
        assert_eq!(
            s.chars().count(),
            RAW_SNIPPET_CHARS + "…(생략)".chars().count()
        );
        assert!(s.ends_with("…(생략)"));
        assert_eq!(snippet("짧다"), "짧다");
    }

    #[test]
    fn api_error_includes_status_and_body() {
        let e = api_error(400, "{\"error\":{\"message\":\"unknown model\"}}", 0).to_string();
        assert!(e.contains("400"), "{e}");
        assert!(e.contains("unknown model"), "{e}");
    }

    // ── 재시도 (§15) ────────────────────────────────────────────────────

    #[test]
    fn only_429_and_5xx_retry() {
        assert!(is_retryable_status(429));
        assert!(is_retryable_status(500));
        assert!(is_retryable_status(503));
        for s in [400u16, 401, 403, 404, 422] {
            assert!(!is_retryable_status(s), "{s} 는 재시도하면 안 된다");
        }
        assert!(!is_retryable_status(200));
    }

    #[test]
    fn backoff_grows_and_stays_bounded() {
        for seed in [0u64, 1, 7, 0xDEAD_BEEF] {
            let d: Vec<u64> = (0..MAX_RETRIES)
                .map(|a| backoff_delay_ms(a, seed))
                .collect();
            // 500 / 1000 / 2000 기준 + 최대 절반의 jitter.
            assert!((500..=750).contains(&d[0]), "{d:?}");
            assert!((1000..=1500).contains(&d[1]), "{d:?}");
            assert!((2000..=3000).contains(&d[2]), "{d:?}");
            assert!(
                d[0] < d[1] && d[1] < d[2],
                "지수적으로 늘어나야 한다: {d:?}"
            );
        }
    }

    #[test]
    fn backoff_jitter_actually_varies() {
        // jitter가 상수면 동시 작업이 같은 순간에 몰려 429를 다시 맞는다.
        let a = backoff_delay_ms(1, 1);
        let b = backoff_delay_ms(1, 2);
        let c = backoff_delay_ms(1, 3);
        assert!(a != b || b != c, "jitter가 씨앗에 반응하지 않는다");
        // 같은 씨앗은 같은 값 — 테스트가 흔들리지 않는다.
        assert_eq!(a, backoff_delay_ms(1, 1));
    }

    #[test]
    fn backoff_is_capped() {
        assert!(backoff_delay_ms(30, 12345) <= BACKOFF_CAP_MS);
        assert!(backoff_delay_ms(u32::MAX, 1) <= BACKOFF_CAP_MS);
    }

    #[test]
    fn retry_after_header_is_honored_in_seconds() {
        assert_eq!(parse_retry_after("12"), Some(12_000));
        assert_eq!(parse_retry_after(" 3 "), Some(3_000));
        assert_eq!(parse_retry_after("99999"), Some(BACKOFF_CAP_MS));
        // HTTP-date 형식은 해석하지 않는다 → 백오프로 넘어간다.
        assert_eq!(parse_retry_after("Wed, 21 Oct 2026 07:28:00 GMT"), None);
    }

    #[test]
    fn jitter_seed_is_time_based_not_rand_crate() {
        // 씨앗이 0으로 고정되면 jitter가 사라진다.
        assert_ne!(jitter_seed(), 0);
        assert_ne!(splitmix64(0), splitmix64(1));
    }

    // ── 실제 네트워크 (오프라인 CI에서는 돌지 않는다) ───────────────────

    #[tokio::test]
    #[ignore = "실제 API 호출. SOUL_E2E=1 과 OPENAI_API_KEY 가 필요하다"]
    async fn e2e_list_models() {
        if std::env::var("SOUL_E2E").is_err() {
            eprintln!("SOUL_E2E 가 없어 건너뛴다");
            return;
        }
        let key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY");
        let c = OpenAi::new(&Config::default(), key).unwrap();
        let models = c.list_models().await.unwrap();
        assert!(!models.is_empty());
        let mut sorted = models.clone();
        sorted.sort();
        assert_eq!(models, sorted);
    }

    #[tokio::test]
    #[ignore = "실제 API 호출. SOUL_E2E=1 과 OPENAI_API_KEY 가 필요하다"]
    async fn e2e_embed_returns_requested_dims() {
        if std::env::var("SOUL_E2E").is_err() {
            return;
        }
        let key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY");
        let c = OpenAi::new(&Config::default(), key).unwrap();
        let v = c
            .embed(
                "text-embedding-3-small",
                256,
                &["차갑고 정돈된 실내".into()],
            )
            .await
            .unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].len(), 256, "§20.2 — dimensions 가 반영되어야 한다");
    }
}
