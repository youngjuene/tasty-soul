//! §10의 structured output 스키마 (JSON Schema, strict).
//!
//! **스키마를 코드에서 조립한다.** 프롬프트와 달리 이것은 파일로 빼지 않는다 —
//! `prompt_sha256`은 프롬프트 텍스트에만 걸리며, 스키마는 코드 버전과 함께 움직인다.
//!
//! 같은 이유로 §11 툴콜 루프가 주고받는 **와이어 형태**(툴콜 파싱·assistant 메시지
//! 정규화)도 여기 둔다. 비평·성찰 두 루프가 같은 규율을 쓰므로 한 곳에만 적는다.

use serde_json::{json, Value};

/// §10.1 — 서술 생성 (이미지·오디오·텍스트·병합 공통).
pub fn describe() -> Value {
    json!({
      "type": "object",
      "additionalProperties": false,
      "required": ["prose", "axes", "tags"],
      "properties": {
        "prose": { "type": "string", "minLength": 8, "maxLength": 120 },
        "axes": {
          "type": "object",
          "additionalProperties": false,
          "required": ["chroma","luminance","density","grain","tempo","space","valence","intensity"],
          "properties": {
            "chroma":    { "type": "number", "minimum": 0, "maximum": 1 },
            "luminance": { "type": "number", "minimum": 0, "maximum": 1 },
            "density":   { "type": "number", "minimum": 0, "maximum": 1 },
            "grain":     { "type": "number", "minimum": 0, "maximum": 1 },
            "tempo":     { "type": "number", "minimum": 0, "maximum": 1 },
            "space":     { "type": "number", "minimum": 0, "maximum": 1 },
            "valence":   { "type": "number", "minimum": 0, "maximum": 1 },
            "intensity": { "type": "number", "minimum": 0, "maximum": 1 }
          }
        },
        "tags": { "type": "array", "maxItems": 6, "items": { "type": "string" } }
      }
    })
}

/// §10.4 — 비평 생성.
pub fn critique() -> Value {
    json!({
      "type": "object",
      "additionalProperties": false,
      "required": ["critique", "lineage"],
      "properties": {
        "critique": { "type": "string", "minLength": 80, "maxLength": 600 },
        "lineage":  { "type": "array", "maxItems": 4, "items": { "type": "string" } }
      }
    })
}

/// §10.1 출력 검증 — 스키마를 통과해도 추가로 확인한다.
///
/// T13의 판정 기준 셋을 그대로 코드로 옮긴 것이다.
/// - `prose`의 **문장 수가 3개 이상이면 1회 재시도**한다 (T27).
///   재시도도 실패하면 그대로 받아들이고 트레이스에 기록한다.
/// - `prose`를 쉼표로 분리했을 때 **각 조각이 전부 명사구면** 분류문이다 (T13 판정 1).
/// - `prose`가 `tags`를 쉼표로 이어붙인 것과 사실상 같으면 재시도 대상이다 (분류문 회귀, §18-8).
/// - **재시도는 최대 1회.** 무한 루프를 만들지 않는다.
pub fn needs_describe_retry(prose: &str, tags: &[String]) -> Option<&'static str> {
    // 순서는 §10.1의 서술 순서를 따른다 — 문장 수를 먼저 본다.
    if count_sentences(prose) >= 3 {
        return Some("prose가 3문장 이상입니다. 한국어 1~2문장으로 다시 쓰세요");
    }
    // tags와 무관하게 성립하는 판정이므로 tags 대조보다 먼저 본다.
    // tags가 비어 있어도(=대조할 것이 없어도) 나열은 나열이다.
    if looks_like_noun_phrase_list(prose) {
        return Some("prose가 쉼표로 이은 명사구 나열입니다. 해석을 담은 문장으로 다시 쓰세요");
    }
    if looks_like_tag_list(prose, tags) {
        return Some("prose가 tags를 이어붙인 분류문입니다. 해석을 담은 문장으로 다시 쓰세요");
    }
    None
}

/// §10.4 검증 — `critique`에 마크다운 문법(`#`, `-`, `*`, `|`)이 포함되면 1회 재시도 (T48).
///
/// `#`·`-`·`*`는 **줄 시작만** 본다. 산문 안의 하이픈("1990-2000")이나 곱셈 기호를
/// 목록으로 오판하면 멀쩡한 비평문이 계속 되돌아온다. 표의 `|`는 산문에 쓸 일이
/// 없으므로 위치를 가리지 않는다.
pub fn needs_critique_retry(critique: &str) -> bool {
    if critique.contains('|') {
        return true;
    }
    critique.lines().any(|line| {
        let t = line.trim_start();
        t.starts_with('#') || t.starts_with('-') || t.starts_with('*')
    })
}

// ───────────────────────────────────────────────────────────── 문장 세기

/// 문장 수. §10.1(1~2문장)과 §11.2(`to_text` 3~6문장)가 같은 정의를 쓴다.
///
/// 종결 부호는 `.` `!` `?` 와 그 전각 짝이다. **끝의 마침표 하나가 문장을 하나 더
/// 만들지 않도록** 종결 부호가 아니라 "부호로 끊긴 내용 조각"의 수를 센다.
/// 숫자 사이의 마침표(`0.5`)는 소수점이므로 종결로 보지 않는다.
pub fn count_sentences(text: &str) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut count = 0usize;
    // 직전 종결 부호 이후로 내용(공백이 아닌 글자)이 있었는가.
    let mut has_content = false;

    for (i, c) in chars.iter().enumerate() {
        if is_terminator(*c) {
            // 소수점은 종결이 아니다.
            if *c == '.'
                && i > 0
                && chars[i - 1].is_ascii_digit()
                && chars.get(i + 1).is_some_and(|n| n.is_ascii_digit())
            {
                has_content = true;
                continue;
            }
            if has_content {
                count += 1;
                has_content = false;
            }
            // 연속 부호("...", "?!")는 하나로 센다.
            continue;
        }
        if !c.is_whitespace() {
            has_content = true;
        }
    }
    // 부호 없이 끝나는 마지막 문장.
    if has_content {
        count += 1;
    }
    count
}

fn is_terminator(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '…' | '。' | '！' | '？')
}

// ─────────────────────────────────────────── §18-8 · T13 분류문 회귀 판정

/// T13 판정 1 — 쉼표로 나눈 조각이 **전부 명사구인가**.
///
/// `tags`를 보지 않는다. 모델은 `tags`와 겹치지 않는 어휘로도 얼마든지 나열할 수
/// 있다 — "흐린 하늘, 낮은 채도, 원경"에는 tags가 한 글자도 필요 없다. `tags` 대조만
/// 하던 판정이 §18-8이 직접 인용한 "채도 낮은 실내, 인물 없음"을 통과시켰던 이유가
/// 이것이다.
///
/// **조각이 하나뿐이면 나열이 아니다.** 쉼표 없는 한 덩어리는 명사구여도
/// 짧은 인상 서술("실내")일 수 있고, 여기서 잡으면 §10.1이 허용한 1문장 서술이
/// 통째로 재시도 대상이 된다.
fn looks_like_noun_phrase_list(prose: &str) -> bool {
    let pieces: Vec<&str> = prose
        .split([',', '，', '、'])
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    if pieces.len() < 2 {
        return false;
    }
    pieces.iter().all(|p| is_noun_phrase(p))
}

/// 조각 하나가 명사구인가 — **한국어 형태소 분석의 실용적 근사다.**
///
/// 조각의 **마지막 어절**만 본다. 한국어는 핵어가 뒤에 오므로 조각의 품사는
/// 마지막 어절이 정한다. 앞쪽에 관형형("채도 **낮은** 실내")이나 조사가 붙은 주어
/// ("저역**이** 강한 반복 비트")가 있어도 끝이 명사면 그 조각은 명사구다.
///
/// 이 규칙이 §6.2의 대표 예시 "차갑고 정돈된, 사람이 방금 지워진 실내"를 지키는
/// 지점이기도 하다. 두 번째 조각은 명사("실내")로 끝나지만 첫 조각이 관형형
/// ("정돈**된**")으로 끝나므로 나열이 아니다. **"쉼표가 있으면 분류문"으로 줄이면
/// 이 문장이 거부되고, 모델은 재시도에서 더 나열에 가까운 문장을 내놓는다.**
fn is_noun_phrase(piece: &str) -> bool {
    let Some(last) = piece.split_whitespace().next_back() else {
        return false;
    };
    let last = last.trim_matches(is_punct);
    if last.is_empty() {
        return false;
    }
    !INFLECTED_ENDINGS.iter().any(|e| last.ends_with(e))
}

/// 마지막 어절이 이것으로 끝나면 **명사구가 아니다** (용언 활용형).
///
/// 두 갈래다.
/// - 종결어미·서술격 조사: `-다` `-요` `-까` `-네` `-군` `-죠` `-구나`
/// - 연결어미·관형사형: `-고` `-며` `-면서` `-어서` `-아서` `-라서` `-지만` `-는데`
///   `-거나` `-된` `-는` `-던` `-한`
///
/// **명사와 겹치는 음절은 일부러 뺐다.** `-면`은 "평면·정면·화면"을, `-서`는
/// "순서·질서"를, `-인`은 "무인"을 용언으로 만들어 버린다. 그래서 `-면서`·`-어서`처럼
/// 겹치지 않는 긴 형태만 남겼다.
///
/// 이 목록에 없는 활용형은 명사구로 새어 나간다(= 분류문을 놓친다). **그쪽이
/// 의도한 방향이다.** 여기서 잘못 잡으면 멀쩡한 해석문이 거부되고, 재시도를 받은
/// 모델은 더 안전한 쪽 — 즉 더 나열에 가까운 쪽 — 으로 물러선다. 놓친 나열은 다음
/// 관측에서 다시 잡을 기회가 있지만, 거부당한 해석문은 그 자리에서 사라진다.
/// 목록을 늘리고 싶어지면 아래 테스트의 `INTERPRETIVE` 표(§17 T13)를 먼저 통과시켜라.
const INFLECTED_ENDINGS: [&str; 20] = [
    // 종결어미·서술격 조사.
    "다", "요", "까", "네", "군", "죠", "구나", //
    // 연결어미.
    "고", "며", "면서", "어서", "아서", "라서", "지만", "는데", "거나", //
    // 관형사형 (뒤에 꾸밀 명사가 없이 조각이 끝난 경우).
    "된", "는", "던", "한",
];

/// §18-8 — `prose`가 `tags` 나열로 회귀했는가.
///
/// 두 판정 중 하나라도 걸리면 분류문으로 본다.
/// 1. 쉼표로 나눈 조각이 **전부** tags 안에 있다 (= tags를 그대로 이어붙였다).
/// 2. prose에서 tags를 빼면 **거의 아무것도 남지 않는다** (= 조사만 붙였다).
fn looks_like_tag_list(prose: &str, tags: &[String]) -> bool {
    let tags: Vec<String> = tags
        .iter()
        .map(|t| squeeze(t))
        .filter(|t| !t.is_empty())
        .collect();
    if tags.is_empty() {
        // 비교 대상이 없으면 판정하지 않는다. 빈 tags로 "전부 포함"이 참이 되면 안 된다.
        return false;
    }

    // 1 — 쉼표 분리.
    let pieces: Vec<String> = prose
        .split([',', '，', '、'])
        .map(squeeze)
        .filter(|p| !p.is_empty())
        .collect();
    if !pieces.is_empty() && pieces.iter().all(|p| tags.contains(p)) {
        return true;
    }

    // 2 — tags를 빼고 남는 양.
    let stripped = squeeze(prose);
    if stripped.is_empty() {
        return false;
    }
    let mut residual = stripped.clone();
    for t in &tags {
        residual = residual.replace(t.as_str(), "");
    }
    let total = stripped.chars().count();
    let left = residual.chars().count();
    // 조사 한둘만 남았거나, 남은 것이 원문의 1/4에 못 미치면 나열이다.
    left <= 2 || left * 4 <= total
}

/// 공백과 문장부호를 걷어낸 비교용 문자열.
fn squeeze(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace() && !is_punct(*c))
        .collect()
}

fn is_punct(c: char) -> bool {
    matches!(
        c,
        '.' | ','
            | '!'
            | '?'
            | '…'
            | '·'
            | '、'
            | '，'
            | '。'
            | '！'
            | '？'
            | '"'
            | '\''
            | '“'
            | '”'
            | '‘'
            | '’'
            | '('
            | ')'
            | '['
            | ']'
            | '-'
            | '—'
            | '~'
            | ':'
            | ';'
            | '/'
    )
}

// ───────────────────────────────────────────── §11 툴콜 루프의 와이어 형태

/// 모델이 요청한 툴 호출 하나.
#[derive(Clone, Debug)]
pub(crate) struct ToolCall {
    pub id: String,
    pub name: String,
    /// 항상 파싱이 끝난 JSON. 문자열로 온 `arguments`도 여기서 풀어 둔다.
    pub args: Value,
}

/// 한 턴의 모델 응답.
#[derive(Clone, Debug, Default)]
pub(crate) struct Turn {
    pub content: Option<String>,
    pub calls: Vec<ToolCall>,
}

impl Turn {
    /// 대화 이력에 쌓을 assistant 메시지.
    ///
    /// `tool_turn`이 Chat Completions 형태로 주든 Responses 형태로 주든 **이력에는
    /// 정규화된 형태만 넣는다.** 루프가 제공자 응답 모양에 끌려다니지 않게 한다.
    pub fn as_message(&self) -> Value {
        let mut m = serde_json::Map::new();
        m.insert("role".into(), json!("assistant"));
        m.insert(
            "content".into(),
            self.content
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        if !self.calls.is_empty() {
            let calls: Vec<Value> = self
                .calls
                .iter()
                .map(|c| {
                    json!({
                        "id": c.id,
                        "type": "function",
                        "function": { "name": c.name, "arguments": c.args.to_string() }
                    })
                })
                .collect();
            m.insert("tool_calls".into(), Value::Array(calls));
        }
        Value::Object(m)
    }
}

/// `OpenAi::tool_turn`의 반환값에서 다음 수를 읽는다.
///
/// Chat Completions(`choices[0].message` · `message` · 벌거벗은 message)와
/// Responses(`output[]`)를 모두 받는다. 어느 쪽이든 실패하면 툴콜 없는 턴이 되어
/// 루프가 조용히 끝난다 — 추측으로 채우지 않는다.
pub(crate) fn parse_turn(v: &Value) -> Turn {
    if let Some(m) = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
    {
        return parse_turn(m);
    }
    if let Some(m) = v.get("message") {
        return parse_turn(m);
    }

    let mut out = Turn {
        content: v
            .get("content")
            .and_then(text_of)
            .or_else(|| v.get("output_text").and_then(text_of)),
        calls: Vec::new(),
    };

    if let Some(arr) = v.get("tool_calls").and_then(|x| x.as_array()) {
        out.calls.extend(arr.iter().filter_map(parse_call));
    }
    if let Some(arr) = v.get("output").and_then(|x| x.as_array()) {
        for item in arr {
            if item.get("type").and_then(|t| t.as_str()) == Some("function_call") {
                if let Some(tc) = parse_call(item) {
                    out.calls.push(tc);
                }
            } else if out.content.is_none() {
                out.content = item.get("content").and_then(text_of);
            }
        }
    }
    out
}

fn parse_call(v: &Value) -> Option<ToolCall> {
    let f = v.get("function").unwrap_or(v);
    let name = f.get("name").and_then(|x| x.as_str())?.to_string();
    let id = v
        .get("id")
        .and_then(|x| x.as_str())
        .or_else(|| v.get("call_id").and_then(|x| x.as_str()))
        .unwrap_or("call")
        .to_string();
    let raw = f
        .get("arguments")
        .or_else(|| f.get("args"))
        .cloned()
        .unwrap_or(Value::Null);
    let args = match raw {
        // OpenAI는 인자를 JSON **문자열**로 준다.
        Value::String(s) => serde_json::from_str(&s).unwrap_or_else(|_| json!({})),
        Value::Null => json!({}),
        other => other,
    };
    Some(ToolCall { id, name, args })
}

fn text_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Array(a) => {
            let joined: String = a
                .iter()
                .filter_map(|x| x.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("");
            (!joined.is_empty()).then_some(joined)
        }
        _ => None,
    }
}

/// §11.3 — 모든 API 호출을 한 줄씩 남긴다.
///
/// 트레이스 쓰기 실패는 **무시한다.** 재현성의 일부가 아니므로 이것 때문에
/// 에이전트가 멈추면 안 된다.
#[allow(clippy::too_many_arguments)]
pub(crate) fn trace_call(
    tracer: Option<&soul_core::trace::Tracer>,
    purpose: &str,
    model: &str,
    prompt_sha256: Option<&str>,
    tokens_in: u64,
    tokens_out: u64,
    latency_ms: u64,
    response_raw: Option<&str>,
    error: Option<&str>,
) {
    let Some(t) = tracer else { return };
    let _ = t.write(&soul_core::trace::TraceEntry {
        ts: soul_core::time::Ts::now(),
        purpose: purpose.to_string(),
        model: model.to_string(),
        prompt_sha256: prompt_sha256.map(|s| s.to_string()),
        tokens_in,
        tokens_out,
        // 단가표를 코드에 박지 않는다 (§9.8 — 모델 ID조차 설정에서 온다).
        cost_usd_est: 0.0,
        latency_ms,
        response_raw: response_raw.map(|s| s.to_string()),
        error: error.map(|s| s.to_string()),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn sentence_count_ignores_the_final_period() {
        assert_eq!(count_sentences("차갑고 정돈된, 사람이 방금 지워진 실내"), 1);
        assert_eq!(count_sentences("차갑고 정돈된 실내다."), 1);
        assert_eq!(count_sentences("하나다. 둘이다."), 2);
        assert_eq!(count_sentences("하나다. 둘이다. 셋이다."), 3);
        assert_eq!(count_sentences("하나다. 둘이다. 셋이다"), 3);
        assert_eq!(count_sentences("정말? 그렇다! 아니다."), 3);
        // 연속 부호는 하나로 센다.
        assert_eq!(count_sentences("글쎄... 그런가?!"), 2);
        assert_eq!(count_sentences(""), 0);
        assert_eq!(count_sentences("   "), 0);
        // 소수점은 종결이 아니다.
        assert_eq!(count_sentences("노출이 0.5초쯤 열려 있다."), 1);
    }

    /// T27 — 문장이 3개 이상이면 재시도 사유가 돌아온다.
    #[test]
    fn three_or_more_sentences_triggers_retry() {
        let t = tags(&["실내"]);
        assert!(needs_describe_retry("차갑다. 정돈되어 있다. 사람이 없다.", &t).is_some());
        assert!(
            needs_describe_retry("차갑다. 정돈되어 있다. 사람이 없다. 그래서 서늘하다.", &t)
                .is_some()
        );
        // 1~2문장은 통과한다.
        assert_eq!(
            needs_describe_retry("차갑고 정돈된, 사람이 방금 지워진 실내", &t),
            None
        );
        assert_eq!(
            needs_describe_retry("빛이 고여 있다. 아무도 돌아오지 않는다.", &t),
            None
        );
    }

    // ───────────────────────────────── §18-8 · §10.1 · T13 분류문 회귀 판정
    //
    // 아래 두 표가 이 판정의 **계약 전문**이다. 왼쪽(분류문)을 놓치면 감각 층
    // 응답률이 0으로 수렴하고, 오른쪽(해석문)을 잡으면 §6.2의 대표 예시가 거부되어
    // 모델이 재시도에서 더 나열에 가까운 문장을 내놓는다. 둘 다 크래시하지 않는다.

    /// **반드시 잡아야 하는 분류문.** 첫 줄이 §18-8이 직접 인용한 문장이다.
    const CLASSIFICATION: [&str; 7] = [
        "채도 낮은 실내, 인물 없음",
        "무채색 톤, 좁은 공간, 정면 구도",
        "흐린 하늘, 낮은 채도, 원경",
        "저역이 강한 반복 비트, 보컬 없음",
        "고채도, 고밀도, 빠른 템포",
        "실내, 자연광, 무인",
        "무채색, 저밀도, 느림, 평면적, 서늘함, 조용함",
    ];

    /// **절대 오탐하면 안 되는 해석문.** 첫 줄이 §6.2의 예시다.
    const INTERPRETIVE: [&str; 6] = [
        "차갑고 정돈된, 사람이 방금 지워진 실내",
        "자연광이 식탁을 반쯤 지워 놓았다",
        "겨울 아침의 부엌은 아직 아무도 깨우지 않았다",
        "소리가 벽을 통과하지 못하고 방 안에서만 늙는다",
        "정적이 실내를 붙잡고 놓지 않는다",
        "저역이 방을 삼킨다",
    ];

    /// T13 판정 1 — 쉼표 조각이 전부 명사구면 분류문이다.
    ///
    /// **tags를 비워 두고 부른다.** tags가 있으면 "tags를 이어붙였는가" 규칙이
    /// 대신 잡아 줄 수 있어서, 명사구 판정이 실제로 동작하는지 재지 못한다.
    #[test]
    fn classification_prose_is_caught_even_with_no_tags() {
        for prose in CLASSIFICATION {
            assert!(
                needs_describe_retry(prose, &[]).is_some(),
                "분류문을 통과시켰다 (§18-8): {prose:?}"
            );
        }
    }

    /// 해석문은 tags와 겹치든 말든 통과한다.
    #[test]
    fn interpretive_prose_is_never_flagged_as_classification() {
        let t = tags(&["실내", "자연광", "무인"]);
        for prose in INTERPRETIVE {
            assert_eq!(
                needs_describe_retry(prose, &[]),
                None,
                "해석문을 분류문으로 오탐했다 (§6.2): {prose:?}"
            );
            assert_eq!(
                needs_describe_retry(prose, &t),
                None,
                "해석문을 분류문으로 오탐했다 (§6.2): {prose:?}"
            );
        }
    }

    /// T27 — 재시도는 **정확히 1회**로 끝난다.
    ///
    /// 판정이 수렴하려면 "재시도가 받아 오길 바라는 문장"이 반드시 통과해야 한다.
    /// 분류문 → 해석문으로 고쳐 온 응답을 또 거부하면 파이프라인은 (재시도가 1회로
    /// 묶여 있으므로) 무한 루프 대신 **분류문을 그대로 채택**하게 된다.
    #[test]
    fn a_rewritten_classification_passes_so_one_retry_is_enough() {
        let t = tags(&["실내", "저채도", "무인"]);
        assert!(needs_describe_retry(CLASSIFICATION[0], &t).is_some());
        assert_eq!(needs_describe_retry(INTERPRETIVE[0], &t), None);
    }

    /// §18-8 — tags를 쉼표로 이어붙인 분류문은 재시도 대상이다.
    #[test]
    fn tag_list_regression_triggers_retry() {
        let t = tags(&["실내", "자연광", "무인"]);
        // 그대로 이어붙인 경우.
        assert!(needs_describe_retry("실내, 자연광, 무인", &t).is_some());
        // 순서를 바꾸고 조사만 붙인 경우.
        assert!(needs_describe_retry("자연광, 실내, 무인이다.", &t).is_some());
        // 태그 하나뿐인 경우.
        assert!(needs_describe_retry("실내", &t).is_some());
    }

    #[test]
    fn interpretive_prose_passes_even_when_it_reuses_a_tag() {
        let t = tags(&["실내", "자연광", "무인"]);
        assert_eq!(
            needs_describe_retry("차갑고 정돈된, 사람이 방금 지워진 실내", &t),
            None
        );
        assert_eq!(
            needs_describe_retry("자연광이 식탁을 반쯤 지워 놓았다", &t),
            None
        );
        // tags가 비면 판정 자체를 하지 않는다.
        assert_eq!(needs_describe_retry("실내", &[]), None);
    }

    /// T48 — 줄 시작의 목록 기호와 표의 파이프를 잡는다.
    #[test]
    fn critique_retry_catches_line_start_markers_and_pipes() {
        assert!(needs_critique_retry("## 계보\n\n산문"));
        assert!(needs_critique_retry("- 슈게이즈\n- 드림 팝"));
        assert!(needs_critique_retry("* 슈게이즈"));
        assert!(needs_critique_retry("  - 들여쓴 불릿"));
        assert!(needs_critique_retry("| 항목 | 값 |"));
        assert!(needs_critique_retry("첫 줄은 산문이다.\n# 그러나 헤더"));
    }

    /// 산문 안의 하이픈을 목록으로 오판하지 않는다.
    #[test]
    fn critique_retry_does_not_flag_prose_hyphens() {
        assert!(!needs_critique_retry(
            "1990-2000년의 영국 기타 음악이 잔향을 악기처럼 다루던 어법을 따른다. \
             그 계보가 기대던 벽 같은 볼륨 대신 여백을 남기는 쪽이다."
        ));
        assert!(!needs_critique_retry(
            "소리의 크기가 아니라 사라지는 속도로 공간을 만든다."
        ));
        assert!(!needs_critique_retry("전-후 대비가 뚜렷하다."));
        assert!(!needs_critique_retry(""));
    }

    #[test]
    fn parse_turn_reads_chat_completions_shape() {
        let v = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "web_search", "arguments": "{\"query\":\"슈게이즈\"}" }
                    }]
                }
            }]
        });
        let t = parse_turn(&v);
        assert_eq!(t.calls.len(), 1);
        assert_eq!(t.calls[0].name, "web_search");
        assert_eq!(t.calls[0].id, "call_1");
        assert_eq!(t.calls[0].args["query"], "슈게이즈");
        // 이력에는 정규화된 형태만 들어간다.
        let m = t.as_message();
        assert_eq!(m["role"], "assistant");
        assert_eq!(m["tool_calls"][0]["function"]["name"], "web_search");
    }

    #[test]
    fn parse_turn_reads_responses_shape_and_plain_text() {
        let v = json!({
            "output": [
                { "type": "function_call", "call_id": "fc_1", "name": "finish",
                  "arguments": { "critique": "산문", "lineage": ["드림 팝"] } }
            ]
        });
        let t = parse_turn(&v);
        assert_eq!(t.calls.len(), 1);
        assert_eq!(t.calls[0].id, "fc_1");
        assert_eq!(t.calls[0].args["lineage"][0], "드림 팝");

        // 툴콜 없이 말만 하는 턴.
        let t2 = parse_turn(&json!({ "role": "assistant", "content": "근거를 찾지 못했습니다" }));
        assert!(t2.calls.is_empty());
        assert_eq!(t2.content.as_deref(), Some("근거를 찾지 못했습니다"));
        assert_eq!(t2.as_message()["content"], "근거를 찾지 못했습니다");
    }

    #[test]
    fn schemas_match_spec_shape() {
        let d = describe();
        assert_eq!(d["properties"]["prose"]["maxLength"], 120);
        assert_eq!(d["properties"]["tags"]["maxItems"], 6);
        assert_eq!(d["additionalProperties"], false);
        let c = critique();
        assert_eq!(c["properties"]["critique"]["minLength"], 80);
        assert_eq!(c["properties"]["lineage"]["maxItems"], 4);
    }
}
