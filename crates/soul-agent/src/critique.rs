//! 비평 에이전트 (§11.1) — `context` 관측을 만드는 검색 기반 루프.
//!
//! | 툴 | 시그니처 |
//! |---|---|
//! | `web_search` | `(query) -> [{title, url, snippet}]` |
//! | `web_fetch` | `(url) -> {title, text, url}` — 본문 최대 4000자, `url`은 리다이렉트 뒤 최종 주소 |
//! | `finish` | `(critique, lineage)` **종결** |
//!
//! 한계: `agent_max_turns_critique`턴 / `web_search` 최대 3회 /
//! 전체 `agent_timeout_critique_secs`초.
//!
//! **모든 호출의 `query`와 방문한 URL을 빠짐없이 기록한다** (T47).
//!
//! `sources`가 2건 미만이면 `grounded: false`로 기록되고 화면 2.1이 경고를 띄운다.
//! **아무 근거도 못 찾으면 실패로 종료하고 `context` 관측을 만들지 않는다** (T52).
//! 추측으로 채운 비평은 사용자의 선호 판단을 오염시킨다 (§18-7).
//!
//! 세는 단위는 URL 문자열이 아니라 **페이지**다 (`absorb_page`): 본문이 빈 페이지는
//! 근거가 아니고, 정규화한 최종 URL이 같으면 몇 번을 열어도 1건이다.
//! 검색을 한 번도 하지 않은(=`queries`가 빈) 실행도 관측을 만들지 않는다 (§D6).

use crate::schemas;
use serde_json::{json, Value};
use soul_core::error::Result;
use soul_core::obs::{ContextObs, Kind, ModelRef, SourceRef};
use soul_core::prompts::{self, PromptFile};
use soul_core::time::Ts;
use soul_net::search::FetchedPage;
use std::time::{Duration, Instant};

/// §10.4의 `kind`별 추가 입력.
#[derive(Debug)]
pub struct CritiqueInput {
    pub target_id: soul_core::ids::ObsId,
    pub kind: Kind,
    pub machine_prose: String,
    pub tags: Vec<String>,
    /// YouTube면 제목·채널·설명·태그. 로컬 영상이면 파일명. 텍스트면 원문(최대 4000자).
    pub extra: Option<String>,
}

pub struct CritiqueOutcome {
    /// 근거를 못 찾았으면 `None` — **관측을 만들지 않는다.**
    pub context: Option<ContextObs>,
    pub turns: usize,
    pub searches: usize,
}

/// 마크다운으로 회귀한 `finish`를 되돌려보낼 때 쓰는 사유 (§10.4, T48).
const MARKDOWN_RETRY: &str = "비평문에 마크다운 문법(줄 시작의 #·-·*, 또는 |)이 있습니다. \
목록·헤더·표를 쓰지 말고 이어지는 산문으로 다시 써서 finish를 다시 부르세요.";

/// `web_fetch` 한 건의 상한. 페이지 하나가 전체 예산을 다 쓰지 못하게 한다.
fn fetch_timeout_secs(total: u64) -> u64 {
    total.clamp(5, 30)
}

pub async fn run(
    input: &CritiqueInput,
    client: &soul_net::OpenAi,
    model: &str,
    search_cfg: &soul_core::config::SearchConfig,
    search_key: Option<&str>,
    thresholds: &soul_core::config::Thresholds,
    tracer: Option<&soul_core::trace::Tracer>,
) -> Result<CritiqueOutcome> {
    // §R11 — 기록하는 해시는 **렌더링이 끝난 최종 시스템 프롬프트**의 것이다.
    // 대상별 정보는 시스템 프롬프트가 아니라 첫 사용자 메시지로 간다. 그렇게 해야
    // 해시가 "프롬프트가 바뀐 지점"만 가리킨다.
    let system = render_system(&prompts::load(PromptFile::Critique)?, thresholds);
    let prompt_sha256 = prompts::sha256_of(&system);
    let tool_defs = tools();

    let mut messages: Vec<Value> = vec![json!({ "role": "user", "content": user_brief(input) })];
    let mut queries: Vec<String> = Vec::new();
    let mut sources: Vec<SourceRef> = Vec::new();
    let mut calls: Vec<String> = Vec::new();
    let mut turns = 0usize;
    let mut searches = 0usize;
    let mut finish: Option<Finish> = None;
    let mut markdown_retried = false;

    let max_turns = thresholds.agent_max_turns_critique.max(1);
    let max_searches = thresholds.agent_max_searches_critique;
    let budget = Duration::from_secs(thresholds.agent_timeout_critique_secs.max(1));
    let fetch_secs = fetch_timeout_secs(thresholds.agent_timeout_critique_secs);

    // 누산기를 블록 **밖**에 두었으므로, 타임아웃으로 이 future가 버려져도
    // 그때까지 모은 query·source는 남는다 (T47의 기록이 부분적으로라도 살아야 한다).
    let loop_fut = async {
        while turns < max_turns {
            turns += 1;

            let started = Instant::now();
            let call = match client
                .tool_turn(model, &system, &messages, &tool_defs)
                .await
            {
                Ok(c) => {
                    schemas::trace_call(
                        tracer,
                        "critique",
                        model,
                        Some(&prompt_sha256),
                        c.tokens_in,
                        c.tokens_out,
                        c.latency_ms,
                        Some(&c.raw),
                        None,
                    );
                    c
                }
                Err(e) => {
                    schemas::trace_call(
                        tracer,
                        "critique",
                        model,
                        Some(&prompt_sha256),
                        0,
                        0,
                        started.elapsed().as_millis() as u64,
                        None,
                        Some(&e.to_string()),
                    );
                    return Err(e);
                }
            };
            calls.push(call.call_id.clone());

            let turn = schemas::parse_turn(&call.value);
            messages.push(turn.as_message());
            if turn.calls.is_empty() {
                // 종결 툴 없이 말만 하는 턴 = 근거를 못 찾았다는 뜻이다. 여기서 끝낸다.
                break;
            }

            for tc in &turn.calls {
                let content = match tc.name.as_str() {
                    "web_search" => {
                        let q = str_arg(&tc.args, "query");
                        if q.is_empty() {
                            "query가 비어 있습니다.".to_string()
                        } else if searches >= max_searches {
                            format!(
                                "검색 횟수 상한({max_searches}회)에 도달했습니다. \
                                 지금까지 web_fetch로 연 페이지만으로 finish를 부르거나, 근거가 없다고 답하세요."
                            )
                        } else {
                            searches += 1;
                            // §D6 — 실제로 나간 질의를 빠짐없이 남긴다. 프라이버시 고지의 근거다 (T47).
                            queries.push(q.clone());
                            let t0 = Instant::now();
                            match soul_net::search::web_search(search_cfg, search_key, &q).await {
                                Ok(hits) => {
                                    schemas::trace_call(
                                        tracer,
                                        "critique.web_search",
                                        &search_cfg.provider,
                                        None,
                                        0,
                                        0,
                                        t0.elapsed().as_millis() as u64,
                                        None,
                                        None,
                                    );
                                    let arr: Vec<Value> = hits
                                        .iter()
                                        .map(|h| {
                                            json!({"title": h.title, "url": h.url, "snippet": h.snippet})
                                        })
                                        .collect();
                                    json!({ "results": arr }).to_string()
                                }
                                Err(e) => {
                                    schemas::trace_call(
                                        tracer,
                                        "critique.web_search",
                                        &search_cfg.provider,
                                        None,
                                        0,
                                        0,
                                        t0.elapsed().as_millis() as u64,
                                        None,
                                        Some(&e.to_string()),
                                    );
                                    json!({ "error": e.to_string() }).to_string()
                                }
                            }
                        }
                    }
                    "web_fetch" => {
                        let url = str_arg(&tc.args, "url");
                        if url.is_empty() {
                            "url이 비어 있습니다.".to_string()
                        } else {
                            let t0 = Instant::now();
                            match soul_net::search::web_fetch(&url, fetch_secs).await {
                                Ok(page) => {
                                    schemas::trace_call(
                                        tracer,
                                        "critique.web_fetch",
                                        &url,
                                        None,
                                        0,
                                        0,
                                        t0.elapsed().as_millis() as u64,
                                        None,
                                        None,
                                    );
                                    // **실제로 연 페이지만 근거로 센다.** 검색 결과 목록의 URL은
                                    // 사용자가 "근거"로 확인할 수 있는 것이 아니다 (§6.4).
                                    absorb_page(&mut sources, &url, &page)
                                }
                                Err(e) => {
                                    schemas::trace_call(
                                        tracer,
                                        "critique.web_fetch",
                                        &url,
                                        None,
                                        0,
                                        0,
                                        t0.elapsed().as_millis() as u64,
                                        None,
                                        Some(&e.to_string()),
                                    );
                                    json!({ "error": e.to_string() }).to_string()
                                }
                            }
                        }
                    }
                    "finish" => match parse_finish(&tc.args) {
                        Err(msg) => msg,
                        Ok(f) => {
                            // §10.4 검증 — 마크다운이면 1회만 되돌려보낸다.
                            if !markdown_retried && schemas::needs_critique_retry(&f.critique) {
                                markdown_retried = true;
                                MARKDOWN_RETRY.to_string()
                            } else {
                                finish = Some(f);
                                "기록했습니다.".to_string()
                            }
                        }
                    },
                    other => format!("알 수 없는 툴입니다: {other}"),
                };
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": tc.id,
                    "content": content
                }));
                if finish.is_some() {
                    break;
                }
            }

            if finish.is_some() {
                break;
            }
        }
        Ok::<(), soul_core::error::SoulError>(())
    };

    // §11.1 — 전체 예산. 넘기면 지금까지 모은 것으로 판정한다(대개 `context: None`이다).
    match tokio::time::timeout(budget, loop_fut).await {
        Ok(r) => r?,
        Err(_) => schemas::trace_call(
            tracer,
            "critique",
            model,
            Some(&prompt_sha256),
            0,
            0,
            budget.as_millis() as u64,
            None,
            Some("agent_timeout_critique_secs 초과"),
        ),
    }

    let context = build_context(
        input,
        finish,
        queries,
        sources,
        model,
        &prompt_sha256,
        calls,
    );
    Ok(CritiqueOutcome {
        context,
        turns,
        searches,
    })
}

// ─────────────────────────────────────────────────────────────── 프롬프트

/// §R11 — 렌더링이 끝난 최종 시스템 프롬프트.
///
/// 파일(`prompts/critique.md`)에 없는 것은 **한계 수치와 근거 규칙**뿐이다.
/// 둘 다 설정에서 오므로 파일에 박아 둘 수 없다.
fn render_system(base: &str, thresholds: &soul_core::config::Thresholds) -> String {
    let mut s = base.trim_end().to_string();
    s.push_str("\n\n## 한계와 근거\n\n");
    s.push_str(&format!(
        "- `web_search`는 최대 {}회, 전체 {}턴, {}초 안에 끝낸다.\n",
        thresholds.agent_max_searches_critique,
        thresholds.agent_max_turns_critique,
        thresholds.agent_timeout_critique_secs
    ));
    s.push_str(
        "- **근거로 세는 것은 `web_fetch`로 실제로 연 페이지뿐이다.** \
         검색 결과 목록은 근거가 아니다. 사용자가 링크를 눌러 확인할 수 있어야 한다.\n",
    );
    s.push_str(
        "- 검색 결과 중 믿을 만한 것을 **최소 2건** `web_fetch`로 열어 확인한 뒤 `finish`를 부른다. \
         2건에 못 미치면 화면에 \"근거 부족\" 경고가 붙는다.\n",
    );
    s.push_str(
        "- **같은 페이지는 몇 번을 열어도 1건이다.** 주소의 `#조각`이나 `utm_` 같은 추적 \
         파라미터만 다른 URL, 리다이렉트로 같은 글에 도착하는 URL은 모두 한 건으로 센다. \
         본문을 읽지 못한 페이지(빈 페이지)도 근거가 아니다 — 다른 페이지를 열어야 한다.\n",
    );
    s.push_str(
        "- 연 페이지가 하나도 없거나 `web_search`를 한 번도 부르지 않았으면 이 비평은 \
         **기록되지 않는다.** 그럴 때는 `finish`를 부르지 말고 근거를 찾지 못했다고 답한다.\n",
    );
    s.push('\n');
    s
}

/// 대상별 입력 (§10.4의 `kind`별 표). **시스템 프롬프트가 아니라 사용자 메시지다.**
fn user_brief(input: &CritiqueInput) -> String {
    let mut s = String::new();
    s.push_str("## 대상\n\n");
    s.push_str(&format!("- kind: {}\n", input.kind.as_str()));
    s.push_str(&format!(
        "- machine.prose: {}\n",
        input.machine_prose.trim()
    ));
    s.push_str(&format!("- tags: {}\n", input.tags.join(", ")));
    if let Some(extra) = input
        .extra
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        s.push_str("\n## 추가 입력\n\n");
        s.push_str(extra);
        s.push('\n');
    }
    match input.kind {
        // §10.4 — 고유명사가 없는 대상은 특정할 수 없는 것이 정상이다.
        Kind::Image | Kind::Text => s.push_str(
            "\n특정할 고유명사가 없으면 근거를 찾지 못하는 것이 정상이다. \
             억지로 지어내지 말고 그렇게 답한다.\n",
        ),
        Kind::Audio | Kind::Video => {
            s.push_str("\n제목과 채널명이 가장 확실한 실마리다. 그것부터 검색한다.\n")
        }
    }
    s
}

/// §11.1의 툴 3개. OpenAI function calling 정의.
fn tools() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "web_search",
                "description": "검색어로 결과 목록을 받는다. 결과 목록만으로는 근거가 되지 않는다.",
                "parameters": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["query"],
                    "properties": {
                        "query": { "type": "string", "description": "실제로 검색할 문자열" }
                    }
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "web_fetch",
                "description": "페이지 본문을 최대 4000자 읽는다. 여기서 연 페이지만 근거로 센다.",
                "parameters": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["url"],
                    "properties": {
                        "url": { "type": "string", "description": "검색 결과에서 고른 절대 URL" }
                    }
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "finish",
                "description": "비평문을 확정하고 종결한다. 근거가 하나도 없으면 부르지 않는다.",
                "parameters": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["critique", "lineage"],
                    "properties": {
                        "critique": {
                            "type": "string",
                            "description": "한국어 산문 3~5문장. 목록·헤더·불릿 금지"
                        },
                        "lineage": {
                            "type": "array",
                            "maxItems": 4,
                            "items": { "type": "string" },
                            "description": "속하거나 인접한 계보·사조·장르 0~4개"
                        }
                    }
                }
            }
        }),
    ]
}

// ─────────────────────────────────────────────────────────────── 근거 세기

/// 본문이 비어 모델에게 되돌려주는 사유. **실패를 실패라고 말한다** (§18-7).
const EMPTY_BODY: &str = "이 페이지는 열렸지만 읽을 본문이 없습니다. \
근거로 세지 않았습니다. 다른 페이지를 web_fetch로 여세요.";

/// 이미 센 페이지를 다시 열었을 때의 안내. 남은 턴을 같은 글에 쓰지 않게 한다.
const ALREADY_COUNTED: &str = "이미 근거로 센 페이지입니다(같은 최종 URL). \
근거를 늘리려면 다른 출처를 여세요.";

/// `web_fetch` 결과 한 건을 근거로 받아들일지 정하고, 모델에게 돌려줄 툴 응답을 만든다.
///
/// **§18-7이 말하는 "근거를 무엇으로 세는가"가 정해지는 유일한 지점이다.**
/// `grounded`는 `sources.len() >= 2`로만 정해지므로(§6.4), 이 함수가 무엇을 밀어 넣느냐가
/// 화면 2.1이 사용자에게 하는 말의 진실성을 그대로 결정한다. 두 가지를 막는다.
///
/// 1. **본문이 0자인 페이지는 근거가 아니다.** 404 대신 빈 셸을 주는 사이트, 자바스크립트로만
///    본문을 그리는 사이트는 200을 돌려준다. 그것을 근거로 세면 사용자는 "근거 2건"을 보고
///    링크를 눌러 빈 화면을 만난다. 모델에게는 실패를 그대로 알려 **다른 페이지를 열게** 한다.
///    조용히 성공시키면 모델은 근거를 더 찾을 이유가 없어진다.
/// 2. **같은 페이지는 몇 번을 열어도 한 건이다.** 프래그먼트·추적 쿼리만 다른 URL과
///    리다이렉트로 합류하는 URL을 `canonical_url`로 같은 키에 모은다. 판정 기준은 요청 URL이
///    아니라 **리다이렉트 뒤의 최종 URL**이다 — 단축 링크는 요청 URL만 보면 늘 달라 보인다.
///
/// 이 방어를 지우면 테스트는 통과하고 사용자만 속는다. 지우고 싶어지면 §11.1의
/// "사용자가 링크를 눌러 확인할 수 있어야 한다"를 먼저 읽어라.
fn absorb_page(sources: &mut Vec<SourceRef>, requested: &str, page: &FetchedPage) -> String {
    if page.text.trim().is_empty() {
        return json!({ "error": EMPTY_BODY, "url": requested }).to_string();
    }
    // 최종 URL이 비어 있는 경우(이론상)에만 요청 URL로 물러선다.
    let landed = if page.final_url.trim().is_empty() {
        requested
    } else {
        page.final_url.as_str()
    };
    let key = soul_net::search::canonical_url(landed);
    let duplicate = sources.iter().any(|s| s.url == key);
    if !duplicate {
        // 기록도 정규화한 최종 URL로 남긴다. 사용자가 여는 주소와 우리가 센 단위가
        // 어긋나면 "근거 2건"의 근거를 아무도 검산할 수 없다 (§D6).
        sources.push(SourceRef {
            url: key.clone(),
            title: page.title.clone(),
            fetched_at: Ts::now(),
        });
    }
    let mut out = json!({ "title": page.title, "text": page.text, "url": key });
    if duplicate {
        out["note"] = json!(ALREADY_COUNTED);
    }
    out.to_string()
}

// ───────────────────────────────────────────────────────────── 종결 처리

#[derive(Debug)]
struct Finish {
    critique: String,
    lineage: Vec<String>,
}

fn parse_finish(args: &Value) -> std::result::Result<Finish, String> {
    let critique = str_arg(args, "critique");
    if critique.is_empty() {
        return Err(
            "critique가 비어 있습니다. 검색으로 확인한 것이 있으면 산문으로 쓰고, \
             없으면 finish를 부르지 말고 근거가 없다고 답하세요."
                .to_string(),
        );
    }
    let mut lineage: Vec<String> = args
        .get("lineage")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    // §6.4 — 0~4개. 넘치면 관측 검증에서 걸리므로 여기서 자른다.
    lineage.truncate(4);
    Ok(Finish { critique, lineage })
}

/// 루프 결과를 `context` 관측으로 굳힌다.
///
/// **`sources`가 0이면 `None`이다** (T52·§18-7). `finish`를 불렀더라도 버린다 —
/// 근거 없는 비평은 사용자의 선호 판단을 오염시킨다.
///
/// **`queries`가 0이어도 `None`이다** (§D6·T47). 검색을 한 번도 하지 않고 쓴 비평은
/// 모델이 이미 알고 있다고 믿는 것을 적은 글이지 근거를 확인한 글이 아니다. 게다가
/// `queries`는 프라이버시 고지의 근거이므로, 빈 채로 기록되면 고지가 거짓이 된다.
/// 같은 규칙이 `Observation::validate`에도 있다 — 여기서 걸러야 사용자에게 "근거를 못
/// 찾았다"고 제때 말할 수 있고, 거기서 막아야 다른 경로로 새지 않는다.
fn build_context(
    input: &CritiqueInput,
    finish: Option<Finish>,
    queries: Vec<String>,
    sources: Vec<SourceRef>,
    model: &str,
    prompt_sha256: &str,
    calls: Vec<String>,
) -> Option<ContextObs> {
    let f = finish?;
    if sources.is_empty() {
        return None;
    }
    if queries.iter().all(|q| q.trim().is_empty()) {
        return None;
    }
    let critique = f.critique.trim().to_string();
    if critique.is_empty() {
        return None;
    }
    // §6.4 — grounded는 **파이프라인이 센다.** 모델에게 묻지 않는다.
    let grounded = sources.len() >= 2;
    Some(ContextObs {
        id: soul_core::ids::new_id(),
        ts: Ts::now(),
        schema: soul_core::SCHEMA_VERSION,
        target: input.target_id.clone(),
        critique,
        lineage: f.lineage,
        queries,
        sources,
        grounded,
        model: ModelRef {
            provider: "openai".to_string(),
            id: model.to_string(),
            prompt_sha256: Some(prompt_sha256.to_string()),
            calls,
        },
    })
}

fn str_arg(args: &Value, key: &str) -> String {
    args.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> CritiqueInput {
        CritiqueInput {
            target_id: soul_core::ids::new_id(),
            kind: Kind::Audio,
            machine_prose: "잔향이 먼저 오고 소리가 뒤따른다".into(),
            tags: vec!["잔향".into(), "여백".into()],
            extra: Some("제목: Souvlaki / 채널: Slowdive".into()),
        }
    }

    fn finish() -> Finish {
        Finish {
            critique: "잔향을 악기처럼 다루던 90년대 초 영국 기타 음악의 어법을 따르되, \
                       그 계보가 대개 기댔던 벽 같은 볼륨 대신 여백을 남긴다."
                .into(),
            lineage: vec!["슈게이즈".into(), "드림 팝".into()],
        }
    }

    fn source(url: &str) -> SourceRef {
        SourceRef {
            url: url.into(),
            title: "제목".into(),
            fetched_at: Ts::parse("2026-08-13T09:20:11.000Z").unwrap(),
        }
    }

    /// `web_fetch`가 돌려준 페이지 한 건. `requested`와 `final_url`이 다를 수 있다.
    fn page(final_url: &str, text: &str) -> FetchedPage {
        FetchedPage {
            title: "제목".into(),
            text: text.into(),
            final_url: final_url.into(),
        }
    }

    /// 같은 페이지를 프래그먼트만 다른 URL로 두 번 열면 근거는 **1건**이다 (§18-7).
    ///
    /// 이걸 놓치면 `grounded: true`가 "서로 다른 출처 2곳"이 아니라
    /// "URL 문자열 2개"를 뜻하게 된다. 사용자는 링크를 눌러 같은 글을 두 번 본다.
    #[test]
    fn the_same_page_opened_twice_is_one_source() {
        let mut sources = Vec::new();
        absorb_page(
            &mut sources,
            "https://example.org/article#intro",
            &page("https://example.org/article#intro", "본문"),
        );
        absorb_page(
            &mut sources,
            "https://example.org/article/?utm_source=x#credits",
            &page("https://example.org/article/?utm_source=x#credits", "본문"),
        );
        assert_eq!(sources.len(), 1, "같은 페이지다: {sources:?}");

        let c = build_context(
            &input(),
            Some(finish()),
            vec!["q".into()],
            sources,
            "gpt-x",
            "sha",
            vec![],
        )
        .expect("근거 1건이면 관측은 만들어진다");
        assert!(!c.grounded, "같은 페이지를 두 번 연 것은 근거 2건이 아니다");
    }

    /// 서로 다른 입력 URL이 리다이렉트로 같은 페이지에 합류하면 근거는 **1건**이다.
    #[test]
    fn redirects_that_land_on_one_page_are_one_source() {
        let mut sources = Vec::new();
        absorb_page(
            &mut sources,
            "https://short.example/abc",
            &page("https://example.org/article", "본문"),
        );
        absorb_page(
            &mut sources,
            "https://example.org/article?fbclid=zzz",
            &page("https://example.org/article", "본문"),
        );
        assert_eq!(sources.len(), 1, "최종 URL이 같다: {sources:?}");
        assert_eq!(
            sources[0].url, "https://example.org/article",
            "기록은 최종 URL로 남는다"
        );
    }

    /// 본문이 0자인 페이지는 근거가 아니다. 모델에게는 실패를 그대로 알린다 (§18-7).
    #[test]
    fn pages_with_no_body_are_not_evidence() {
        let mut sources = Vec::new();
        let r1 = absorb_page(
            &mut sources,
            "https://a.example/x",
            &page("https://a.example/x", ""),
        );
        let r2 = absorb_page(
            &mut sources,
            "https://b.example/y",
            &page("https://b.example/y", "   \n  "),
        );
        assert!(
            sources.is_empty(),
            "열리기만 한 빈 페이지는 근거가 아니다: {sources:?}"
        );
        for r in [&r1, &r2] {
            let v: Value = serde_json::from_str(r).expect("툴 응답은 JSON");
            assert!(v.get("error").is_some(), "모델에게 실패를 알려야 한다: {r}");
        }

        let c = build_context(
            &input(),
            Some(finish()),
            vec!["q".into()],
            sources,
            "gpt-x",
            "sha",
            vec![],
        );
        assert!(c.is_none(), "근거가 0건이므로 관측을 만들지 않는다 (T52)");
    }

    /// §D6 — 검색을 한 번도 하지 않은 실행은 관측을 만들지 않는다.
    /// `queries`가 프라이버시 고지의 근거이므로, 비어 있으면 고지가 거짓이 된다 (T47).
    #[test]
    fn a_run_without_any_query_yields_no_observation() {
        let c = build_context(
            &input(),
            Some(finish()),
            vec![],
            vec![source("https://a"), source("https://b")],
            "gpt-x",
            "sha",
            vec![],
        );
        assert!(c.is_none(), "검색 없이 만든 비평은 근거가 없는 것과 같다");
    }

    /// T52 — 근거가 0건이면 `finish`를 불렀어도 관측을 만들지 않는다.
    #[test]
    fn no_sources_yields_no_observation() {
        let c = build_context(
            &input(),
            Some(finish()),
            vec!["슬로우다이브 souvlaki".into()],
            vec![],
            "gpt-x",
            "sha",
            vec!["resp_1".into()],
        );
        assert!(c.is_none(), "근거 없는 비평은 버린다 (§18-7)");
    }

    #[test]
    fn without_finish_there_is_no_observation() {
        let c = build_context(
            &input(),
            None,
            vec!["q".into()],
            vec![source("https://a"), source("https://b")],
            "gpt-x",
            "sha",
            vec![],
        );
        assert!(c.is_none(), "종결 툴을 부르지 않았으면 관측이 없다");
    }

    /// §6.4 — `grounded`는 sources 개수로만 정해진다. 모델에게 묻지 않는다 (T58).
    #[test]
    fn grounded_is_counted_by_the_pipeline() {
        let one = build_context(
            &input(),
            Some(finish()),
            vec!["q".into()],
            vec![source("https://a")],
            "gpt-x",
            "sha",
            vec![],
        )
        .expect("근거 1건이면 관측은 만들어진다");
        assert!(!one.grounded, "1건이면 grounded=false");

        let two = build_context(
            &input(),
            Some(finish()),
            vec!["q".into()],
            vec![source("https://a"), source("https://b")],
            "gpt-x",
            "sha",
            vec![],
        )
        .expect("근거 2건");
        assert!(two.grounded);
        // 관측 불변식(§6.4)을 그대로 통과해야 한다.
        soul_core::obs::Observation::Context(two)
            .validate()
            .unwrap();
    }

    /// T47 — query와 방문 URL이 빠짐없이 실린다.
    #[test]
    fn queries_and_sources_are_carried_into_the_observation() {
        let queries = vec!["slowdive souvlaki".to_string(), "슈게이즈 계보".to_string()];
        let c = build_context(
            &input(),
            Some(finish()),
            queries.clone(),
            vec![source("https://a"), source("https://b")],
            "gpt-x",
            "sha256값",
            vec!["resp_1".into(), "resp_2".into()],
        )
        .expect("관측");
        assert_eq!(c.queries, queries);
        assert_eq!(
            c.sources.iter().map(|s| s.url.as_str()).collect::<Vec<_>>(),
            ["https://a", "https://b"]
        );
        assert_eq!(c.lineage, ["슈게이즈", "드림 팝"]);
        assert_eq!(c.model.prompt_sha256.as_deref(), Some("sha256값"));
        assert_eq!(c.model.calls, ["resp_1", "resp_2"]);
    }

    #[test]
    fn lineage_is_capped_at_four() {
        let f = parse_finish(&json!({
            "critique": "산문",
            "lineage": ["a", "b", "", "c", "d", "e"]
        }))
        .expect("파싱");
        assert_eq!(f.lineage, ["a", "b", "c", "d"]);
    }

    #[test]
    fn empty_critique_is_sent_back_to_the_agent() {
        let msg = parse_finish(&json!({ "critique": "  ", "lineage": [] })).unwrap_err();
        assert!(msg.contains("critique"), "{msg}");
    }

    /// §R11 — 최종 시스템 프롬프트에 한계와 근거 규칙이 들어가고, 해시가 그것을 따른다.
    #[test]
    fn system_prompt_states_limits_and_fetch_rule() {
        let base = prompts::load(PromptFile::Critique).expect("prompts/critique.md");
        let mut th = soul_core::config::Thresholds::default();
        let s = render_system(&base, &th);
        assert!(s.contains("최대 3회"), "{s}");
        assert!(s.contains("web_fetch"), "{s}");
        assert!(s.contains("최소 2건"), "{s}");
        assert!(s.contains("기록되지 않는다"), "{s}");
        // 설정이 바뀌면 최종 프롬프트도 해시도 바뀐다 (§R11).
        th.agent_max_searches_critique = 5;
        assert_ne!(
            prompts::sha256_of(&s),
            prompts::sha256_of(&render_system(&base, &th))
        );
    }

    #[test]
    fn user_brief_carries_target_fields_not_the_system_prompt() {
        let b = user_brief(&input());
        assert!(b.contains("잔향이 먼저 오고"));
        assert!(b.contains("Slowdive"));
        assert!(b.contains("audio"));
        // 대상 정보는 시스템 프롬프트에 들어가지 않는다 — 들어가면 해시가 항목마다 갈린다.
        let s = render_system(
            &prompts::load(PromptFile::Critique).unwrap(),
            &soul_core::config::Thresholds::default(),
        );
        assert!(!s.contains("Slowdive"));
    }

    #[test]
    fn tool_definitions_are_the_three_of_spec() {
        let t = tools();
        let names: Vec<&str> = t
            .iter()
            .map(|d| d["function"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["web_search", "web_fetch", "finish"]);
        for d in &t {
            assert_eq!(d["type"], "function");
            assert_eq!(d["function"]["parameters"]["additionalProperties"], false);
        }
    }

    #[test]
    fn fetch_timeout_is_bounded() {
        assert_eq!(fetch_timeout_secs(90), 30);
        assert_eq!(fetch_timeout_secs(10), 10);
        assert_eq!(fetch_timeout_secs(1), 5);
    }

    /// 실제 루프는 OpenAI와 검색 제공자가 필요하다. CI 기본 경로는 완전 오프라인이다.
    #[tokio::test]
    #[ignore = "네트워크 필요 — SOUL_E2E=1"]
    async fn e2e_critique_loop() {
        if std::env::var("SOUL_E2E").is_err() {
            return;
        }
        let config = soul_core::config::Config::default();
        let key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY");
        let client = soul_net::OpenAi::new(&config, key).expect("클라이언트");
        let out = run(
            &input(),
            &client,
            &config.models.text,
            &config.search,
            None,
            &config.thresholds,
            None,
        )
        .await
        .expect("루프");
        assert!(out.turns >= 1);
        if let Some(c) = out.context {
            assert!(!c.queries.is_empty(), "T47");
            assert!(!c.sources.is_empty(), "T52");
            assert_eq!(c.grounded, c.sources.len() >= 2);
        }
    }
}
