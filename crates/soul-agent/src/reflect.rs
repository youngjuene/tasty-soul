//! 성찰 에이전트 (§11.2).
//!
//! 트리거: 마지막 `soul_delta` 이후 **`type == "ingest"`인 관측이
//! `reflect_trigger_ingests`건** 누적. 또는 사용자 수동 실행 (`soul reflect --force`).
//!
//! ## `window` 정의 (§11.2)
//!
//! - `from` = 마지막 `soul_delta`보다 ULID가 큰 첫 관측의 ID.
//!   `soul_delta`가 하나도 없으면 **전체 관측 중 가장 작은 ID**.
//! - `to` = 제안 시점의 가장 큰 관측 ID.
//! - 두 경계 모두 **포함**(inclusive)이다.
//!
//! | 툴 | 시그니처 |
//! |---|---|
//! | `list_observations` | `(since_id, limit) -> [{id, type, ts, prose_short}]` |
//! | `read_observation` | `(id) -> 전체 JSON` |
//! | `query_stats` | `() -> §12 지표 전부` — **인자 없음** |
//! | `read_soul` | `() -> {block_id: {text, hash}}` — **`soul:human` 제외** (§D4, T29) |
//! | `propose_delta` | `(blocks, axis_delta, cites, rationale)` **종결** |
//!
//! `prose_short` = 해당 관측의 `prose`를 **앞에서 40자**로 자른 것. 없으면 `null`.
//! `list_observations`의 `limit` 상한은 200.
//!
//! ## `propose_delta` 가드레일
//!
//! Rust에서 검증하고, 위반 시 **거부 사유를 툴 결과로 반환해 에이전트가 재시도하게 한다**
//! (T8·T9). 재시도는 턴 상한 안에서만 허용한다.
//!
//! - `axis_delta`의 각 값 `|Δ| ≤ axis_delta_max`
//! - `axis_delta`의 키는 §7의 8축에 한정
//! - `cites` 최소 3개, 전부 `window` 범위 안에 실존하는 ID
//! - `blocks.profile.from_hash`가 현재 해시와 일치
//! - `to_text`가 현재 텍스트와 상이하고, 3~6문장, 한국어
//!
//! **에이전트는 `SOUL.md`를 직접 쓰지 않는다.** `soul/SOUL.next.md`에 쓰고 앱이 diff 뷰를 띄운다.
//! 사용자 승인 시에만 `soul_delta` 기록 → 재렌더 → git 커밋.
//! 거절 시 관측을 기록하지 않고 트레이스에만 남긴다.

use crate::schemas;
use serde_json::{json, Value};
use soul_core::error::{Result, SoulError};
use soul_core::ids::ObsId;
use soul_core::obs::{AxisDelta, BlockDelta, ModelRef, ObsSet, Observation, SoulDelta};
use soul_core::prompts::{self, PromptFile};
use soul_core::soulblocks::{self, BlockView, Destination};
use soul_core::soulmd;
use soul_core::time::Ts;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

pub struct ReflectOutcome {
    /// 제안. 아직 관측이 아니다 — 사용자가 승인해야 기록된다.
    pub proposal: Option<SoulDelta>,
    pub turns: usize,
    /// 가드레일에 걸려 되돌려보낸 횟수.
    pub rejections: Vec<String>,
}

/// §8.2 — 성찰이 손댈 수 있는 유일한 블록. `soul:neg id=profile`.
const PROFILE_BLOCK: &str = "profile";
/// §11.2 — `prose_short`의 길이. **문자 단위다** (바이트가 아니다).
const PROSE_SHORT_CHARS: usize = 40;

/// §11.2 트리거 판정.
pub fn should_trigger(set: &ObsSet, threshold: usize) -> bool {
    set.ingests_since_last_delta() >= threshold
}

/// §11.2의 `window` 계산.
pub fn window(set: &ObsSet) -> Option<soul_core::obs::Window> {
    // `as_slice()`는 ULID 오름차순이다 (`ObsSet::new`가 정렬해 둔다).
    let to = set.as_slice().last()?.id().clone();
    let from = match set.last_soul_delta() {
        // 마지막 델타 **이후**의 첫 관측. 델타 뒤에 아무것도 없으면 볼 창이 없다.
        Some(d) => set.first_after(&d.id)?.id().clone(),
        None => set.as_slice().first()?.id().clone(),
    };
    Some(soul_core::obs::Window { from, to })
}

/// 가드레일 검증. 실패 시 **사람이 읽을 수 있는 사유**를 돌려준다 (에이전트가 읽는다).
#[allow(clippy::too_many_arguments)]
pub fn check_guardrails(
    set: &ObsSet,
    window: &soul_core::obs::Window,
    blocks: &std::collections::BTreeMap<String, soul_core::obs::BlockDelta>,
    axis_delta: &soul_core::obs::AxisDelta,
    cites: &[soul_core::ids::ObsId],
    current_profile_hash: &str,
    current_profile_text: &str,
    axis_delta_max: f64,
) -> std::result::Result<(), String> {
    // ── 조건 2 — 키는 §7의 8축뿐이다. 모르는 축을 먼저 걸러야 아래 검사가 의미를 갖는다.
    for k in axis_delta.keys() {
        if soul_core::obs::Axis::parse(k).is_none() {
            return Err(format!(
                "axis_delta에 알 수 없는 축 `{k}`가 있습니다. 쓸 수 있는 축은 {} 뿐입니다.",
                soul_core::obs::AXIS_NAMES.join(", ")
            ));
        }
    }

    // ── 조건 1 — |Δ| ≤ axis_delta_max.
    for (k, v) in axis_delta {
        if !v.is_finite() {
            return Err(format!("축 `{k}`의 변화량이 수가 아닙니다."));
        }
        // 부동소수 표현 오차로 상한값 자체가 거부되지 않게 한다.
        if v.abs() > axis_delta_max + 1e-9 {
            return Err(format!(
                "축 `{k}`의 변화량 {v:+} 이(가) 한 번에 허용된 폭을 넘습니다. \
                 |Δ| ≤ {axis_delta_max} 이어야 합니다. 여러 번에 나누어 제안하세요."
            ));
        }
    }

    // ── 조건 3 — cites 최소 3개, 전부 window 안에 실존.
    let mut seen: BTreeSet<&ObsId> = BTreeSet::new();
    for id in cites {
        if !seen.insert(id) {
            continue;
        }
        if set.get(id).is_none() {
            return Err(format!(
                "인용한 관측 {id} 은(는) 존재하지 않습니다. list_observations로 실존하는 ID를 확인하세요."
            ));
        }
        if id < &window.from || id > &window.to {
            return Err(format!(
                "인용한 관측 {id} 이(가) window 범위({} ~ {}) 밖입니다. 이 창 안의 관측만 인용할 수 있습니다.",
                window.from, window.to
            ));
        }
    }
    if seen.len() < 3 {
        return Err(format!(
            "인용한 관측이 {}개입니다. 서로 다른 관측 ID를 최소 3개 인용해야 합니다.",
            seen.len()
        ));
    }

    // ── blocks 키 제한. `soul:gen` 블록은 재빌드가 덮어쓰므로 델타로 고칠 수 없다 (§8.1).
    for k in blocks.keys() {
        if k != PROFILE_BLOCK {
            return Err(format!(
                "blocks에 쓸 수 있는 것은 `{PROFILE_BLOCK}` 뿐입니다: `{k}`."
            ));
        }
    }

    // ── 조건 4·5 — profile 블록을 손댈 때만 본다.
    //    "확신이 없으면 profile을 그대로 두고 axis_delta만 제안해도 된다" (§10.5).
    if let Some(b) = blocks.get(PROFILE_BLOCK) {
        if b.from_hash != current_profile_hash {
            return Err(format!(
                "profile 블록의 from_hash `{}` 가 현재 해시 `{}` 와 다릅니다. \
                 read_soul을 다시 불러 최신 해시로 제안하세요.",
                b.from_hash, current_profile_hash
            ));
        }
        let to_text = b.to_text.trim();
        if to_text.is_empty() {
            return Err("profile 블록의 to_text가 비어 있습니다.".to_string());
        }
        if soulmd::normalize_for_hash(to_text) == soulmd::normalize_for_hash(current_profile_text) {
            return Err(
                "profile 블록의 to_text가 현재 텍스트와 같습니다. 달라진 것이 없으면 \
                 blocks를 비우고 axis_delta만 제안하세요."
                    .to_string(),
            );
        }
        let n = schemas::count_sentences(to_text);
        if !(3..=6).contains(&n) {
            return Err(format!(
                "profile 블록의 to_text가 {n}문장입니다. 3~6문장이어야 합니다."
            ));
        }
        if !has_hangul(to_text) {
            return Err("profile 블록의 to_text에 한국어가 없습니다. 한국어로 쓰세요.".to_string());
        }
    }

    // 둘 다 비면 아무것도 제안하지 않은 것이다 — 승인 화면에 띄울 것이 없다.
    if blocks.is_empty() && axis_delta.is_empty() {
        return Err(
            "제안에 변경이 없습니다. axis_delta나 profile 블록 중 적어도 하나는 있어야 합니다."
                .to_string(),
        );
    }

    Ok(())
}

/// 한글 코드포인트(음절·자모)가 하나라도 있는가.
///
/// `pub`인 이유: §11.2의 "한국어" 판정은 여기 한 번만 정의한다. 승인 경로
/// (`soul-pipeline::reflect_flow`)가 "수정 후 승인"의 본문을 같은 기준으로 다시 검사하는데,
/// 거기서 따로 세면 화면 4에서만 통과하는 텍스트가 생긴다 (§R2).
pub fn has_hangul(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(c,
            '\u{AC00}'..='\u{D7A3}' | '\u{1100}'..='\u{11FF}' | '\u{3130}'..='\u{318F}')
    })
}

pub async fn run(
    set: &ObsSet,
    paths: &soul_core::paths::Paths,
    client: &soul_net::OpenAi,
    model: &str,
    thresholds: &soul_core::config::Thresholds,
    derived: &soul_core::derived::Derived,
    tracer: Option<&soul_core::trace::Tracer>,
) -> Result<ReflectOutcome> {
    let Some(win) = window(set) else {
        // 마지막 델타 이후 새 관측이 없다. 볼 것이 없으면 부르지 않는다.
        return Ok(ReflectOutcome {
            proposal: None,
            turns: 0,
            rejections: Vec::new(),
        });
    };

    // §D4·T29 — **`SOUL.md`를 문자열로 읽어 프롬프트에 넣지 않는다.**
    // 반드시 블록 단위로 파싱해 목적지 규칙을 거친다. 여기가 `soul:human`이
    // 원격으로 나가는 것을 막는 유일한 지점이다.
    let md_path = paths.soul_md();
    if !md_path.is_file() {
        return Err(SoulError::MissingPath(md_path));
    }
    let doc = soulmd::parse(&std::fs::read_to_string(&md_path)?)?;
    let view = soulblocks::assemble(&doc, Destination::Remote);
    let (cur_hash, cur_text) = view
        .get(PROFILE_BLOCK)
        .map(|b| (b.hash.clone(), b.text.clone()))
        .unwrap_or_default();

    let system = render_system(&prompts::load(PromptFile::Reflect)?, thresholds);
    let prompt_sha256 = prompts::sha256_of(&system);
    let tool_defs = tools();
    let list_max = thresholds.list_observations_max.max(1);

    let mut messages: Vec<Value> =
        vec![json!({ "role": "user", "content": user_brief(set, &win, &view) })];
    let mut calls: Vec<String> = Vec::new();
    let mut rejections: Vec<String> = Vec::new();
    let mut proposal: Option<SoulDelta> = None;
    let mut turns = 0usize;
    let max_turns = thresholds.agent_max_turns_reflect.max(1);

    while turns < max_turns && proposal.is_none() {
        turns += 1;

        let started = Instant::now();
        let call = match client
            .tool_turn(model, &system, &messages, &tool_defs)
            .await
        {
            Ok(c) => {
                schemas::trace_call(
                    tracer,
                    "reflect",
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
                    "reflect",
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
            // 종결 툴 없이 끝났다 — 제안 없음.
            break;
        }

        for tc in &turn.calls {
            let content = match tc.name.as_str() {
                "list_observations" => list_observations(set, &win, &tc.args, list_max).to_string(),
                "read_observation" => read_observation(set, &tc.args).to_string(),
                "query_stats" => query_stats(set, derived).to_string(),
                // §D4 — 여기도 같은 조립 결과를 쓴다. 파일을 다시 읽지 않는다.
                "read_soul" => serde_json::to_value(&view)
                    .unwrap_or_else(|_| json!({}))
                    .to_string(),
                "propose_delta" => {
                    match parse_proposal(&tc.args).and_then(|p| {
                        check_guardrails(
                            set,
                            &win,
                            &p.blocks,
                            &p.axis_delta,
                            &p.cites,
                            &cur_hash,
                            &cur_text,
                            thresholds.axis_delta_max,
                        )
                        .map(|_| p)
                    }) {
                        // 위반 사유는 **툴 결과로 돌아가** 에이전트가 다시 시도한다 (T8·T9).
                        Err(reason) => {
                            rejections.push(reason.clone());
                            reason
                        }
                        Ok(p) => {
                            proposal = Some(SoulDelta {
                                id: soul_core::ids::new_id(),
                                ts: Ts::now(),
                                schema: soul_core::SCHEMA_VERSION,
                                window: win.clone(),
                                blocks: p.blocks,
                                axis_delta: p.axis_delta,
                                // §6.6 — 항상 null. 스키마 자리만 예약한다.
                                morphology_delta: None,
                                cites: p.cites,
                                rationale: p.rationale,
                                model: ModelRef {
                                    provider: "openai".to_string(),
                                    id: model.to_string(),
                                    prompt_sha256: Some(prompt_sha256.clone()),
                                    calls: calls.clone(),
                                },
                            });
                            "제안을 받았습니다. 사람이 승인 화면에서 판단합니다.".to_string()
                        }
                    }
                }
                other => format!("알 수 없는 툴입니다: {other}"),
            };
            messages.push(json!({
                "role": "tool",
                "tool_call_id": tc.id,
                "content": content
            }));
            if proposal.is_some() {
                break;
            }
        }
    }

    // **`SOUL.md`도 `SOUL.next.md`도 여기서 쓰지 않는다.** 쓰기와 승인은 §13 화면 4를
    // 거치는 `soul-pipeline::reflect_flow`의 몫이다.
    Ok(ReflectOutcome {
        proposal,
        turns,
        rejections,
    })
}

// ─────────────────────────────────────────────────────────────── 프롬프트

/// §10.5·§R11 — 최종 시스템 프롬프트.
///
/// 포함할 것은 파일 본문과 **설정에서 오는 수치뿐이다** (§9.8). 실행마다 달라지는 것은
/// 여기 들어오지 않는다 — 현재 취향 문서도 `window` 경계도 `user_brief`가 첫 사용자
/// 메시지로 싣는다.
///
/// **왜 이 규칙이 있는가 (§R11 · §18-1 · T23).**
/// 이 함수의 결과 sha256이 `soul_delta.model.prompt_sha256`으로 관측에 남고, `soul stats`와
/// 대시보드는 그 값이 바뀌는 지점을 "여기서부터 프롬프트가 달라졌다"는 경계로 그린다.
/// 취향 문서나 `window`를 여기에 섞으면 그 값이 **실행마다** 달라져 경계가 전부 가짜가 되고,
/// 진짜 프롬프트 수정은 그 잡음에 묻힌다. 그러면 프롬프트를 고쳐서 생긴 축 분포의 이동을
/// 사용자가 자기 취향의 변화로 읽게 되며, 이 오류는 사후에 되돌릴 수 없다.
/// 편의상 여기에 상태를 도로 넣고 싶어지겠지만, `read_soul` 툴이 같은 내용을 돌려주므로
/// 시스템 프롬프트의 사본은 애초에 중복이다.
///
/// 반대로 설정 수치(`axis_delta_max` 등)는 남긴다 — 설정을 바꾸면 분포가 실제로 움직이므로
/// 거기에 경계가 생기는 것이 맞다.
fn render_system(base: &str, thresholds: &soul_core::config::Thresholds) -> String {
    // 파일에는 자리표시자만 있다. 실제 상한은 설정에서 온다 (§9.8).
    let mut s = base
        .replace("AXIS_DELTA_MAX", &format!("{}", thresholds.axis_delta_max))
        .trim_end()
        .to_string();

    s.push_str(&format!(
        "\n\n턴 상한은 {}회입니다. 상한 안에서 propose_delta로 끝내세요.\n",
        thresholds.agent_max_turns_reflect
    ));
    s
}

/// 이번 실행의 상태 (§10.5). **시스템 프롬프트가 아니라 사용자 메시지다** — 이유는
/// `render_system`의 주석에 있다 (§R11).
///
/// `view`는 반드시 `assemble(&doc, Destination::Remote)`의 결과다. 그 목적지 규칙이
/// `soul:human`을 애초에 맵에 만들지 않으므로 여기로 옮겨도 유출 경로가 늘지 않는다
/// (§D4·T29). `SOUL.md`를 문자열로 읽어 여기에 붙이는 순간 그 방어가 사라진다.
fn user_brief(
    set: &ObsSet,
    win: &soul_core::obs::Window,
    view: &BTreeMap<String, BlockView>,
) -> String {
    let mut s = format!(
        "관측 {}건(활성 ingest {}건), 마지막 soul_delta 이후 ingest {}건입니다.\n\n",
        set.len(),
        set.active_ingests().len(),
        set.ingests_since_last_delta(),
    );

    s.push_str("## 현재 취향 문서\n\n");
    if view.is_empty() {
        s.push_str("(블록이 없습니다.)\n\n");
    } else {
        for (id, b) in view {
            s.push_str(&format!(
                "### {id} · hash={}\n\n{}\n\n",
                b.hash,
                b.text.trim_end()
            ));
        }
    }

    s.push_str("## window\n\n");
    s.push_str(&format!(
        "from={} · to={} — 양쪽 모두 포함입니다. 인용은 이 범위 안에서만 유효합니다.\n\n",
        win.from, win.to
    ));
    s.push_str(
        "list_observations로 훑고, 필요한 것만 read_observation으로 열고, \
         query_stats로 지표를 본 뒤 propose_delta로 끝내세요.",
    );
    s
}

/// §11.2의 툴 5개.
fn tools() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "list_observations",
                "description": "관측 목록을 훑는다. since_id를 비우면 window의 시작부터 본다.",
                "parameters": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": [],
                    "properties": {
                        "since_id": { "type": "string", "description": "이 ID보다 큰 것만 (배타적)" },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 200 }
                    }
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "read_observation",
                "description": "관측 하나의 전문을 읽는다.",
                "parameters": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["id"],
                    "properties": { "id": { "type": "string" } }
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                // §11.2 — 인자를 받지 않는다. 각 지표는 자기 고유의 창을 이미 갖고 있다.
                "name": "query_stats",
                "description": "파생 지표 전부를 받는다. 인자는 없다.",
                "parameters": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": [],
                    "properties": {}
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "read_soul",
                "description": "현재 취향 문서의 블록들을 {block_id: {text, hash}}로 받는다.",
                "parameters": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": [],
                    "properties": {}
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "propose_delta",
                "description": "변경을 제안하고 종결한다. 가드레일을 어기면 사유가 돌아온다.",
                "parameters": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["axis_delta", "cites", "rationale"],
                    "properties": {
                        "blocks": {
                            "type": "object",
                            "additionalProperties": false,
                            "properties": {
                                "profile": {
                                    "type": "object",
                                    "additionalProperties": false,
                                    "required": ["from_hash", "to_text"],
                                    "properties": {
                                        "from_hash": { "type": "string" },
                                        "to_text": { "type": "string", "description": "한국어 3~6문장" }
                                    }
                                }
                            }
                        },
                        "axis_delta": {
                            "type": "object",
                            "additionalProperties": { "type": "number" },
                            "description": "8축 중 바뀐 것만. 각 |Δ|에 상한이 있다"
                        },
                        "cites": {
                            "type": "array",
                            "minItems": 3,
                            "items": { "type": "string" },
                            "description": "근거 관측 ID. window 안에 실존해야 한다"
                        },
                        "rationale": { "type": "string" }
                    }
                }
            }
        }),
    ]
}

// ───────────────────────────────────────────────────────────────── 툴 구현

/// `(since_id, limit) -> [{id, type, ts, prose_short}]`
fn list_observations(
    set: &ObsSet,
    win: &soul_core::obs::Window,
    args: &Value,
    max: usize,
) -> Value {
    let since = args
        .get("since_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| ObsId::parse(s).ok());
    // §11.2 — limit 상한은 `list_observations_max`(200)다.
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(max)
        .clamp(1, max);

    let items: Vec<Value> = set
        .as_slice()
        .iter()
        .filter(|o| match since.as_ref() {
            Some(s) => o.id() > s,
            // 기본 시작점은 창의 왼쪽 경계다 (포함).
            None => o.id() >= &win.from,
        })
        .take(limit)
        .map(|o| {
            json!({
                "id": o.id().to_string(),
                "type": o.type_name(),
                "ts": o.ts().to_rfc3339_millis(),
                "prose_short": prose_short(o),
            })
        })
        .collect();

    json!({ "observations": items, "limit": limit })
}

/// `prose`의 앞 **40자**(문자 단위). `prose`가 없는 타입이면 `null`.
fn prose_short(o: &Observation) -> Value {
    match o.prose() {
        Some(p) => Value::String(p.chars().take(PROSE_SHORT_CHARS).collect()),
        None => Value::Null,
    }
}

fn read_observation(set: &ObsSet, args: &Value) -> Value {
    let Some(raw) = args.get("id").and_then(|v| v.as_str()) else {
        return json!({ "error": "id가 없습니다." });
    };
    let Ok(id) = ObsId::parse(raw.trim()) else {
        return json!({ "error": format!("{raw} 은(는) ULID 형식이 아닙니다.") });
    };
    match set.get(&id) {
        Some(o) => serde_json::to_value(o).unwrap_or_else(|e| json!({ "error": e.to_string() })),
        None => json!({ "error": format!("관측 {id} 을(를) 찾을 수 없습니다.") }),
    }
}

/// §12 지표 전부. **인자를 받지 않는다** (§11.2).
fn query_stats(set: &ObsSet, derived: &soul_core::derived::Derived) -> Value {
    let stats = soul_core::derived::stats::build(derived, set, None);
    serde_json::to_value(stats).unwrap_or_else(|e| json!({ "error": e.to_string() }))
}

// ───────────────────────────────────────────────────────── propose_delta

struct Proposal {
    blocks: BTreeMap<String, BlockDelta>,
    axis_delta: AxisDelta,
    cites: Vec<ObsId>,
    rationale: String,
}

/// 인자를 구조로 옮긴다. 형태가 틀리면 **가드레일과 같은 방식으로** 사유를 돌려준다.
fn parse_proposal(args: &Value) -> std::result::Result<Proposal, String> {
    let mut blocks = BTreeMap::new();
    match args.get("blocks") {
        None | Some(Value::Null) => {}
        Some(Value::Object(map)) => {
            for (k, v) in map {
                let from_hash = v
                    .get("from_hash")
                    .and_then(|x| x.as_str())
                    .ok_or_else(|| format!("blocks.{k}.from_hash가 없습니다."))?
                    .trim()
                    .to_string();
                let to_text = v
                    .get("to_text")
                    .and_then(|x| x.as_str())
                    .ok_or_else(|| format!("blocks.{k}.to_text가 없습니다."))?
                    .to_string();
                blocks.insert(k.clone(), BlockDelta { from_hash, to_text });
            }
        }
        Some(_) => return Err("blocks는 객체여야 합니다.".to_string()),
    }

    let mut axis_delta: AxisDelta = AxisDelta::new();
    match args.get("axis_delta") {
        None | Some(Value::Null) => {}
        Some(Value::Object(map)) => {
            for (k, v) in map {
                let n = v
                    .as_f64()
                    .ok_or_else(|| format!("axis_delta.{k}는 수여야 합니다."))?;
                axis_delta.insert(k.clone(), n);
            }
        }
        Some(_) => return Err("axis_delta는 객체여야 합니다.".to_string()),
    }

    let mut cites = Vec::new();
    if let Some(arr) = args.get("cites").and_then(|v| v.as_array()) {
        for c in arr {
            let s = c
                .as_str()
                .ok_or_else(|| "cites의 항목은 문자열 ID여야 합니다.".to_string())?;
            let id = ObsId::parse(s.trim())
                .map_err(|_| format!("cites의 `{s}` 는 ULID 형식이 아닙니다."))?;
            cites.push(id);
        }
    }

    let rationale = args
        .get("rationale")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    if rationale.is_empty() {
        return Err(
            "rationale이 비어 있습니다. 무엇이 반복되어 이 제안을 하는지 한 문장으로 쓰세요."
                .to_string(),
        );
    }

    Ok(Proposal {
        blocks,
        axis_delta,
        cites,
        rationale,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use soul_core::obs::{Axes, Ingest, Kind, Machine, Quality, Source, Verdict, Window};

    const ID1: &str = "01AAAAAAAAAAAAAAAAAAAAAAAA";
    const ID2: &str = "01BBBBBBBBBBBBBBBBBBBBBBBB";
    const ID3: &str = "01CCCCCCCCCCCCCCCCCCCCCCCC";
    const ID4: &str = "01DDDDDDDDDDDDDDDDDDDDDDDD";
    const ID5: &str = "01EEEEEEEEEEEEEEEEEEEEEEEE";
    const ID9: &str = "01ZZZZZZZZZZZZZZZZZZZZZZZZ";

    fn id(s: &str) -> ObsId {
        ObsId::parse(s).expect("ULID")
    }

    fn ts(s: &str) -> Ts {
        Ts::parse(s).expect("ts")
    }

    fn model() -> ModelRef {
        ModelRef {
            provider: "test".into(),
            id: "m".into(),
            prompt_sha256: None,
            calls: vec![],
        }
    }

    fn ingest(i: &str, at: &str, prose: &str) -> Observation {
        Observation::Ingest(Ingest {
            id: id(i),
            ts: ts(at),
            schema: soul_core::SCHEMA_VERSION,
            source: Source {
                kind: Kind::Image,
                sha256: "0".into(),
                origin: "file:///x.jpg".into(),
                bytes: 1,
                mime: "image/jpeg".into(),
            },
            machine: Machine {
                prose: prose.into(),
                axes: Axes::from_array([0.4; 8]),
                tags: vec![],
                quality: Quality::Full,
                prompt_sha256: "p1".into(),
            },
            min_dist: None,
            surprisal: 1.0,
            model: model(),
            supersedes: None,
        })
    }

    fn delta(i: &str, at: &str, from: &str, to: &str) -> Observation {
        Observation::SoulDelta(SoulDelta {
            id: id(i),
            ts: ts(at),
            schema: soul_core::SCHEMA_VERSION,
            window: Window {
                from: id(from),
                to: id(to),
            },
            blocks: BTreeMap::new(),
            axis_delta: AxisDelta::new(),
            morphology_delta: None,
            cites: vec![],
            rationale: "이전 제안".into(),
            model: model(),
        })
    }

    fn reading(i: &str, at: &str, target: &str) -> Observation {
        Observation::Reading(soul_core::obs::Reading {
            id: id(i),
            ts: ts(at),
            schema: soul_core::SCHEMA_VERSION,
            layer: soul_core::obs::Layer::Sensory,
            target: id(target),
            verdict: Verdict::Yes,
            prose: None,
            divergence: None,
        })
    }

    /// soul_delta가 없으면 창은 전체다.
    #[test]
    fn window_without_any_delta_spans_everything() {
        let set = ObsSet::new(vec![
            ingest(ID1, "2026-06-01T00:00:00.000Z", "가"),
            ingest(ID2, "2026-06-02T00:00:00.000Z", "나"),
            ingest(ID3, "2026-06-03T00:00:00.000Z", "다"),
        ]);
        let w = window(&set).expect("창");
        assert_eq!(w.from, id(ID1), "전체 관측 중 가장 작은 ID");
        assert_eq!(w.to, id(ID3), "가장 큰 관측 ID");
    }

    /// soul_delta가 있으면 그 **이후**의 첫 관측부터다. 양쪽 모두 포함이다.
    #[test]
    fn window_after_a_delta_starts_at_the_next_observation() {
        let set = ObsSet::new(vec![
            ingest(ID1, "2026-06-01T00:00:00.000Z", "가"),
            delta(ID2, "2026-06-02T00:00:00.000Z", ID1, ID1),
            ingest(ID3, "2026-06-03T00:00:00.000Z", "다"),
            ingest(ID4, "2026-06-04T00:00:00.000Z", "라"),
        ]);
        let w = window(&set).expect("창");
        assert_eq!(w.from, id(ID3), "델타보다 ULID가 큰 첫 관측");
        assert_eq!(w.to, id(ID4));
    }

    #[test]
    fn window_is_none_when_there_is_nothing_new() {
        assert!(window(&ObsSet::default()).is_none());
        let set = ObsSet::new(vec![
            ingest(ID1, "2026-06-01T00:00:00.000Z", "가"),
            delta(ID2, "2026-06-02T00:00:00.000Z", ID1, ID1),
        ]);
        assert!(window(&set).is_none(), "델타 뒤에 관측이 없으면 창이 없다");
    }

    fn fixture() -> (ObsSet, Window) {
        let set = ObsSet::new(vec![
            ingest(ID1, "2026-06-01T00:00:00.000Z", "가"),
            ingest(ID2, "2026-06-02T00:00:00.000Z", "나"),
            ingest(ID3, "2026-06-03T00:00:00.000Z", "다"),
            ingest(ID4, "2026-06-04T00:00:00.000Z", "라"),
        ]);
        let w = window(&set).expect("창");
        (set, w)
    }

    fn cites3() -> Vec<ObsId> {
        vec![id(ID1), id(ID2), id(ID3)]
    }

    fn axes(k: &str, v: f64) -> AxisDelta {
        let mut m = AxisDelta::new();
        m.insert(k.to_string(), v);
        m
    }

    fn ok_call(
        set: &ObsSet,
        w: &Window,
        ad: &AxisDelta,
        cites: &[ObsId],
    ) -> std::result::Result<(), String> {
        check_guardrails(
            set,
            w,
            &BTreeMap::new(),
            ad,
            cites,
            "a3f91c",
            "현재 문장.",
            0.15,
        )
    }

    /// T8 — |Δaxis| = 0.2 는 거부되고 사유가 돌아온다.
    #[test]
    fn t8_axis_delta_over_the_cap_is_rejected() {
        let (set, w) = fixture();
        let err = ok_call(&set, &w, &axes("grain", 0.2), &cites3()).unwrap_err();
        assert!(err.contains("grain"), "{err}");
        assert!(err.contains("0.15"), "상한을 사유에 적어야 한다: {err}");

        // 음수도 절댓값으로 본다.
        assert!(ok_call(&set, &w, &axes("grain", -0.2), &cites3()).is_err());
        // 상한값 자체는 통과한다.
        assert!(ok_call(&set, &w, &axes("grain", 0.15), &cites3()).is_ok());
        assert!(ok_call(&set, &w, &axes("grain", 0.04), &cites3()).is_ok());
    }

    /// T9 — cites가 2개면 거부한다.
    #[test]
    fn t9_two_cites_are_rejected() {
        let (set, w) = fixture();
        let err = ok_call(&set, &w, &axes("grain", 0.04), &[id(ID1), id(ID2)]).unwrap_err();
        assert!(err.contains("2개"), "{err}");
        assert!(err.contains("3개"), "{err}");

        // 같은 ID를 세 번 적어도 근거 3개가 아니다.
        let dup = vec![id(ID1), id(ID1), id(ID1)];
        assert!(ok_call(&set, &w, &axes("grain", 0.04), &dup).is_err());
    }

    #[test]
    fn unknown_axis_is_rejected() {
        let (set, w) = fixture();
        let err = ok_call(&set, &w, &axes("novelty", 0.01), &cites3()).unwrap_err();
        assert!(err.contains("novelty"), "{err}");
        assert!(err.contains("chroma"), "쓸 수 있는 축을 알려야 한다: {err}");
    }

    #[test]
    fn cites_outside_the_window_or_missing_are_rejected() {
        // 델타 이후로 창이 좁아진 상태.
        let set = ObsSet::new(vec![
            ingest(ID1, "2026-06-01T00:00:00.000Z", "가"),
            delta(ID2, "2026-06-02T00:00:00.000Z", ID1, ID1),
            ingest(ID3, "2026-06-03T00:00:00.000Z", "다"),
            ingest(ID4, "2026-06-04T00:00:00.000Z", "라"),
            ingest(ID5, "2026-06-05T00:00:00.000Z", "마"),
        ]);
        let w = window(&set).expect("창");

        // ID1은 실존하지만 창 밖이다.
        let err =
            ok_call(&set, &w, &axes("grain", 0.01), &[id(ID1), id(ID3), id(ID4)]).unwrap_err();
        assert!(err.contains("window"), "{err}");

        // ID9는 아예 없다.
        let err =
            ok_call(&set, &w, &axes("grain", 0.01), &[id(ID9), id(ID3), id(ID4)]).unwrap_err();
        assert!(err.contains("존재하지 않"), "{err}");

        assert!(ok_call(&set, &w, &axes("grain", 0.01), &[id(ID3), id(ID4), id(ID5)]).is_ok());
    }

    fn block(from_hash: &str, to_text: &str) -> BTreeMap<String, BlockDelta> {
        let mut m = BTreeMap::new();
        m.insert(
            PROFILE_BLOCK.to_string(),
            BlockDelta {
                from_hash: from_hash.into(),
                to_text: to_text.into(),
            },
        );
        m
    }

    const CURRENT: &str = "인공물보다 방치된 것을 고른다. 채도가 아니라 습도로 공간을 읽는다.";
    const NEXT: &str = "인공물보다 방치된 것을 고른다. 채도가 아니라 습도로 공간을 읽는다. \
                        사람이 방금 나간 자리에 반복해서 멈춘다.";

    fn with_block(
        set: &ObsSet,
        w: &Window,
        b: &BTreeMap<String, BlockDelta>,
    ) -> std::result::Result<(), String> {
        check_guardrails(
            set,
            w,
            b,
            &AxisDelta::new(),
            &cites3(),
            "a3f91c",
            CURRENT,
            0.15,
        )
    }

    #[test]
    fn from_hash_mismatch_is_rejected() {
        let (set, w) = fixture();
        let err = with_block(&set, &w, &block("deadbe", NEXT)).unwrap_err();
        assert!(err.contains("from_hash"), "{err}");
        assert!(
            err.contains("read_soul"),
            "다시 읽으라고 알려야 한다: {err}"
        );
        assert!(with_block(&set, &w, &block("a3f91c", NEXT)).is_ok());
    }

    #[test]
    fn identical_text_is_rejected() {
        let (set, w) = fixture();
        let err = with_block(&set, &w, &block("a3f91c", CURRENT)).unwrap_err();
        assert!(err.contains("같습니다"), "{err}");
        // 앞뒤 공백만 다른 것도 같은 텍스트다 (§8.3 규칙 3의 정규화).
        let padded = format!("  {CURRENT}  \n");
        assert!(with_block(&set, &w, &block("a3f91c", &padded)).is_err());
    }

    #[test]
    fn to_text_must_be_three_to_six_korean_sentences() {
        let (set, w) = fixture();
        // 2문장.
        let short = "인공물보다 방치된 것을 고른다. 습도로 공간을 읽는다.";
        let err = with_block(&set, &w, &block("a3f91c", short)).unwrap_err();
        assert!(err.contains("2문장"), "{err}");

        // 7문장.
        let long = "하나다. 둘이다. 셋이다. 넷이다. 다섯이다. 여섯이다. 일곱이다.";
        assert!(with_block(&set, &w, &block("a3f91c", long)).is_err());

        // 한국어가 없다.
        let english = "One thing. Another thing. A third thing.";
        let err = with_block(&set, &w, &block("a3f91c", english)).unwrap_err();
        assert!(err.contains("한국어"), "{err}");

        assert!(with_block(&set, &w, &block("a3f91c", NEXT)).is_ok());
    }

    #[test]
    fn only_the_profile_block_can_be_proposed() {
        let (set, w) = fixture();
        let mut m = BTreeMap::new();
        m.insert(
            "axes".to_string(),
            BlockDelta {
                from_hash: "a3f91c".into(),
                to_text: "표".into(),
            },
        );
        let err = with_block(&set, &w, &m).unwrap_err();
        assert!(err.contains("profile"), "{err}");
    }

    #[test]
    fn an_empty_proposal_is_rejected() {
        let (set, w) = fixture();
        let err = check_guardrails(
            &set,
            &w,
            &BTreeMap::new(),
            &AxisDelta::new(),
            &cites3(),
            "a3f91c",
            CURRENT,
            0.15,
        )
        .unwrap_err();
        assert!(err.contains("변경이 없습니다"), "{err}");
    }

    /// §11.2 — `prose_short`는 앞 40**자**다. 바이트로 자르면 한국어가 깨진다.
    #[test]
    fn prose_short_is_forty_characters_not_bytes() {
        let long: String = "가".repeat(60);
        let o = ingest(ID1, "2026-06-01T00:00:00.000Z", &long);
        let v = prose_short(&o);
        let s = v.as_str().expect("문자열");
        assert_eq!(s.chars().count(), 40);
        assert_eq!(s, "가".repeat(40));

        // prose가 없는 타입은 null이다.
        assert_eq!(
            prose_short(&reading(ID2, "2026-06-02T00:00:00.000Z", ID1)),
            Value::Null
        );
    }

    #[test]
    fn list_observations_starts_at_the_window_and_caps_the_limit() {
        let set = ObsSet::new(vec![
            ingest(ID1, "2026-06-01T00:00:00.000Z", "가"),
            delta(ID2, "2026-06-02T00:00:00.000Z", ID1, ID1),
            ingest(ID3, "2026-06-03T00:00:00.000Z", "다"),
            ingest(ID4, "2026-06-04T00:00:00.000Z", "라"),
            ingest(ID5, "2026-06-05T00:00:00.000Z", "마"),
        ]);
        let w = window(&set).expect("창");

        // since_id 없음 → window.from 부터 (포함).
        let v = list_observations(&set, &w, &json!({}), 200);
        let ids: Vec<&str> = v["observations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, [ID3, ID4, ID5]);
        assert_eq!(v["observations"][0]["type"], "ingest");
        assert_eq!(v["observations"][0]["prose_short"], "다");

        // since_id는 배타적이다.
        let v = list_observations(&set, &w, &json!({ "since_id": ID3 }), 200);
        assert_eq!(v["observations"].as_array().unwrap().len(), 2);

        // limit 상한을 넘길 수 없다.
        let v = list_observations(&set, &w, &json!({ "limit": 9999 }), 200);
        assert_eq!(v["limit"], 200);
        let v = list_observations(&set, &w, &json!({ "limit": 1 }), 200);
        assert_eq!(v["observations"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn read_observation_reports_missing_ids_instead_of_failing() {
        let (set, _) = fixture();
        let v = read_observation(&set, &json!({ "id": ID1 }));
        assert_eq!(v["type"], "ingest");
        assert!(read_observation(&set, &json!({ "id": ID9 }))["error"].is_string());
        assert!(read_observation(&set, &json!({ "id": "쓰레기" }))["error"].is_string());
        assert!(read_observation(&set, &json!({}))["error"].is_string());
    }

    #[test]
    fn parse_proposal_returns_readable_reasons() {
        assert!(parse_proposal(&json!({ "rationale": "" }))
            .err()
            .unwrap()
            .contains("rationale"));
        assert!(parse_proposal(&json!({
            "rationale": "습기 관련 정정이 3회 반복됨",
            "cites": ["쓰레기"]
        }))
        .err()
        .unwrap()
        .contains("ULID"));
        assert!(parse_proposal(&json!({
            "rationale": "x",
            "axis_delta": { "grain": "0.04" }
        }))
        .err()
        .unwrap()
        .contains("수여야"));

        let p = parse_proposal(&json!({
            "rationale": "습기·잔향 관련 정정이 3회 반복됨",
            "axis_delta": { "grain": 0.04, "space": -0.02 },
            "cites": [ID1, ID2, ID3],
            "blocks": { "profile": { "from_hash": "a3f91c", "to_text": NEXT } }
        }))
        .expect("파싱");
        assert_eq!(p.cites.len(), 3);
        assert_eq!(p.axis_delta["grain"], 0.04);
        assert_eq!(p.blocks[PROFILE_BLOCK].from_hash, "a3f91c");
    }

    /// T29 — 성찰 호출로 나가는 어느 메시지에도 `soul:human` 블록 내용이 들어가면 안 된다
    /// (§D4·§18-4). 취향 문서가 시스템 프롬프트에서 사용자 메시지로 옮겨졌으므로 **둘 다** 본다.
    #[test]
    fn t29_no_message_ever_carries_the_human_block() {
        const HUMAN: &str = "아무한테도 안 보여줄 문장. 습도라는 말이 자꾸 걸린다.";
        let raw = format!(
            "# SOUL\n\n\
             <!-- soul:neg id=profile rev=1 hash=a3f91c -->\n{CURRENT}\n<!-- /soul:neg -->\n\n\
             ## 내가 쓴 것\n\n\
             <!-- soul:human -->\n{HUMAN}\n<!-- /soul:human -->\n"
        );
        let doc = soulmd::parse(&raw).expect("파싱");
        // 유출을 막는 것은 이 한 줄이다 — Remote 목적지는 human 키를 아예 만들지 않는다.
        let view = soulblocks::assemble(&doc, Destination::Remote);
        assert!(!view.contains_key(soulblocks::HUMAN_KEY));

        let (set, w) = fixture();
        let base = prompts::load(PromptFile::Reflect).expect("prompts/reflect.md");
        let s = render_system(&base, &soul_core::config::Thresholds::default());
        let u = user_brief(&set, &w, &view);

        for (label, msg) in [("system", &s), ("user", &u)] {
            assert!(
                !msg.contains(HUMAN),
                "soul:human이 {label} 메시지로 샜다:\n{msg}"
            );
            assert!(!msg.contains("습도라는 말이"), "{label}:\n{msg}");
        }

        // 나가야 하는 것은 사용자 메시지로 나간다.
        assert!(u.contains(CURRENT), "{u}");
        assert!(
            u.contains("hash="),
            "from_hash 대조를 하려면 해시가 보여야 한다"
        );
        assert!(u.contains(w.from.as_str()) && u.contains(w.to.as_str()));

        // 자리표시자가 실제 상한으로 치환된다 (§R11 — 렌더링이 끝난 최종 프롬프트).
        assert!(!s.contains("AXIS_DELTA_MAX"), "{s}");
        assert!(s.contains("0.15"), "{s}");
        // 실행마다 달라지는 것은 시스템 프롬프트에 남아 있지 않다 (§R11 — 아래 T23 테스트 참고).
        assert!(
            !s.contains(CURRENT),
            "취향 문서가 시스템 프롬프트에 남았다:\n{s}"
        );
        assert!(
            !s.contains(w.from.as_str()),
            "window가 시스템 프롬프트에 남았다:\n{s}"
        );
    }

    /// 한 번의 성찰 실행이 보는 상태.
    struct Run {
        set: ObsSet,
        win: soul_core::obs::Window,
        view: BTreeMap<String, BlockView>,
    }

    /// 실행마다 달라지는 상태 두 벌. 취향 문서도 window도 다르다.
    fn two_runs() -> (Run, Run) {
        fn view(hash: &str, text: &str) -> BTreeMap<String, BlockView> {
            let raw = format!(
                "# SOUL\n\n<!-- soul:neg id=profile rev=1 hash={hash} -->\n{text}\n<!-- /soul:neg -->\n"
            );
            soulblocks::assemble(&soulmd::parse(&raw).expect("파싱"), Destination::Remote)
        }
        let (set_a, w_a) = fixture();
        // 관측이 더 쌓이고 델타가 한 번 반영되어 창도 프로필도 움직였다.
        let set_b = ObsSet::new(vec![
            ingest(ID1, "2026-06-01T00:00:00.000Z", "가"),
            delta(ID2, "2026-06-02T00:00:00.000Z", ID1, ID1),
            ingest(ID3, "2026-06-03T00:00:00.000Z", "다"),
            ingest(ID9, "2026-06-09T00:00:00.000Z", "자"),
        ]);
        let w_b = window(&set_b).expect("창");
        (
            Run {
                set: set_a,
                win: w_a,
                view: view("a3f91c", CURRENT),
            },
            Run {
                set: set_b,
                win: w_b,
                view: view(
                    "bb2200",
                    "완전히 다른 문장이 되었다. 두 번째 문장. 세 번째 문장.",
                ),
            },
        )
    }

    /// §R11·§18-1·T23 — `prompt_sha256`은 **프롬프트가 바뀌었는가**를 판별하는 값이다.
    ///
    /// 실행마다 달라지는 것(현재 취향 문서, window 경계)이 시스템 프롬프트에 들어가면
    /// sha가 매 실행 갈린다. 그러면 경계 목록이 전부 가짜가 되고, 진짜 프롬프트 수정은
    /// 그 잡음에 묻혀 보이지 않는다 — 사용자는 그 이동을 자기 취향의 변화로 읽는다.
    #[test]
    fn t23_reflect_prompt_sha_tracks_the_prompt_file_not_the_run() {
        // 실제 `prompts/*.md`를 임시 디렉토리에 복사해 고친다. 조작한 sha 상수로는
        // "파일을 고치면 sha가 바뀐다"는 성질 자체를 잴 수 없다.
        let src = soul_core::paths::prompts_dir().expect("워크스페이스의 prompts/");
        let tmp = std::env::temp_dir().join(format!(
            "tasty-soul-reflect-prompt-{}-{}",
            std::process::id(),
            soul_core::ids::new_id()
        ));
        std::fs::create_dir_all(&tmp).expect("임시 디렉토리");
        for e in std::fs::read_dir(&src).expect("prompts/ 를 읽을 수 있어야 한다") {
            let p = e.expect("디렉토리 항목").path();
            if p.extension().and_then(|x| x.to_str()) == Some("md") {
                std::fs::copy(&p, tmp.join(p.file_name().expect("파일명"))).expect("복사");
            }
        }
        let file = tmp.join(PromptFile::Reflect.file_name());
        let base = std::fs::read_to_string(&file).expect("복사본 읽기");
        // 복사본이 번들 파일과 바이트까지 같아야 아래 비교가 의미를 갖는다.
        assert_eq!(
            base,
            prompts::load(PromptFile::Reflect).expect("prompts/reflect.md"),
            "복사본이 번들 파일과 달라졌다"
        );

        let th = soul_core::config::Thresholds::default();
        let (a, b) = two_runs();
        let brief_a = user_brief(&a.set, &a.win, &a.view);
        let brief_b = user_brief(&b.set, &b.win, &b.view);
        // 두 실행이 실제로 다른 상태인지부터 확인한다 — 같으면 아래 비교가 공짜로 통과한다.
        assert_ne!(brief_a, brief_b, "두 실행의 사용자 메시지는 달라야 한다");

        let system = render_system(&base, &th);
        let sha_a = prompts::sha256_of(&system);
        // 상태는 시스템 프롬프트를 **인자로도 내용으로도** 건드리지 못한다.
        // 이 목록에 걸리는 것을 시스템 프롬프트로 되돌리는 순간 sha는 매 실행 갈린다.
        for (run, brief) in [(&a, &brief_a), (&b, &brief_b)] {
            for needle in [
                run.view["profile"].text.trim(),
                run.view["profile"].hash.as_str(),
                run.win.from.as_str(),
                run.win.to.as_str(),
            ] {
                assert!(
                    !system.contains(needle),
                    "실행마다 달라지는 `{needle}` 이(가) 시스템 프롬프트에 있다 (§R11):\n{system}"
                );
                // 대신 사용자 메시지로는 반드시 나간다 — 옮기다가 흘리면 안 된다.
                assert!(
                    brief.contains(needle),
                    "`{needle}` 이(가) 사용자 메시지에 없다:\n{brief}"
                );
            }
        }

        // 프롬프트 파일을 한 글자 고치면 sha는 반드시 바뀐다 — 그것이 진짜 경계다.
        let edited = base.replacen("당신은", "당신도", 1);
        assert_ne!(edited, base, "치환이 실제로 일어나야 한다");
        std::fs::write(&file, &edited).expect("수정본 쓰기");
        let base2 = std::fs::read_to_string(&file).expect("수정본 읽기");
        assert_ne!(
            sha_a,
            prompts::sha256_of(&render_system(&base2, &th)),
            "프롬프트 파일이 바뀌었는데 sha가 같다"
        );

        // 설정에서 오는 수치는 시스템 프롬프트에 남는다 — 설정 변경도 분포를 움직이므로
        // 여기에 경계가 생기는 것이 맞다.
        let th2 = soul_core::config::Thresholds {
            axis_delta_max: 0.3,
            ..soul_core::config::Thresholds::default()
        };
        assert_ne!(
            sha_a,
            prompts::sha256_of(&render_system(&base, &th2)),
            "axis_delta_max가 바뀌면 최종 프롬프트도 sha도 바뀌어야 한다"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn should_trigger_counts_ingests_since_the_last_delta() {
        let set = ObsSet::new(vec![
            ingest(ID1, "2026-06-01T00:00:00.000Z", "가"),
            delta(ID2, "2026-06-02T00:00:00.000Z", ID1, ID1),
            ingest(ID3, "2026-06-03T00:00:00.000Z", "다"),
            ingest(ID4, "2026-06-04T00:00:00.000Z", "라"),
        ]);
        assert!(should_trigger(&set, 2));
        assert!(!should_trigger(&set, 3));
    }

    /// 실제 루프는 OpenAI가 필요하다. CI 기본 경로는 완전 오프라인이다.
    #[tokio::test]
    #[ignore = "네트워크 필요 — SOUL_E2E=1"]
    async fn e2e_reflect_loop() {
        if std::env::var("SOUL_E2E").is_err() {
            return;
        }
        let paths = soul_core::paths::Paths::discover().expect("경로");
        let config = soul_core::config::Config::load(&paths.config_toml()).expect("설정");
        let key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY");
        let client = soul_net::OpenAi::new(&config, key).expect("클라이언트");
        let set = soul_core::obs::Store::new(paths.clone())
            .load_set()
            .expect("관측");
        let derived = soul_core::derived::Derived::default();
        let out = run(
            &set,
            &paths,
            &client,
            &config.models.reflect,
            &config.thresholds,
            &derived,
            None,
        )
        .await
        .expect("루프");
        assert!(out.turns <= config.thresholds.agent_max_turns_reflect);
    }
}
