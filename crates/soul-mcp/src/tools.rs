//! 툴 (§19.4).
//!
//! | 툴 | 시그니처 |
//! |---|---|
//! | `soul_recall` | `(tags?, kind?, since?, until?, limit=20) -> [관측 요약]` |
//! | `soul_similar` | `(observation_id, n=5) -> [관측 요약]` — 캐시 벡터끼리 코사인. **API 호출 없음** (T33) |
//! | `soul_read` | `(observation_id) -> 관측 전문` |
//! | `soul_examples` | `(axis, pole, n=5) -> [관측 요약]` — `pole`은 `high` \| `low` |
//! | `soul_readings` | `(layer?, verdict?, cell?, limit=20) -> [{target, layer, verdict, prose}]` |
//! | `soul_context` | `(ingest_id) -> {critique, lineage, sources}` — 최신 하나 |
//! | `soul_timeline` | `(month?) -> state_at` — 생략 시 전체 |
//!
//! ## 스키마 예산 (§19.2.1)
//!
//! - 툴 설명문은 **한 줄, 120자 이내**로 쓴다 (T37)
//! - 파라미터 설명은 이름으로 자명한 경우 생략한다
//! - 툴을 늘리고 싶어지면 기존 툴에 인자를 추가하는 쪽을 먼저 검토한다
//!
//! ## 관측 요약 형식
//!
//! `{id, ts, kind, prose, tags, surprisal}`. `prose`는 `prose_max_chars`로 자르고,
//! 한 번에 반환하는 건수는 `max_recall_limit`을 넘지 않는다.
//! **`axes`와 임베딩은 요약에 넣지 않는다** (토큰 낭비).

use crate::server::Ctx;
use anyhow::{anyhow, bail};
use serde_json::{json, Value};
use soul_core::db::embed_cache::Space;
use soul_core::derived::divergence::cell_of;
use soul_core::derived::{state, Cell};
use soul_core::ids::ObsId;
use soul_core::obs::{Axis, Ingest, Kind, Layer, ObsSet, Reading, Verdict, AXIS_NAMES};
use soul_core::time::{Month, Ts};
use soul_core::vecmath;
use std::collections::BTreeMap;

/// 노출하는 툴 전부. **쓰기 툴은 하나도 없다** (§19.6, T34).
pub const NAMES: [&str; 7] = [
    "soul_recall",
    "soul_similar",
    "soul_read",
    "soul_examples",
    "soul_readings",
    "soul_context",
    "soul_timeline",
];

/// **쓰기 툴이 없다는 것이 이 목록의 요점이다** (T34).
pub fn list() -> Value {
    json!({
        "tools": [
            {
                "name": "soul_recall",
                "description": "태그·종류·기간으로 관측을 찾는다. 임베딩이 필요 없는 진입점이며 여기서 얻은 id 로 다른 툴을 부른다",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "tags": { "type": "array", "items": { "type": "string" },
                                  "description": "모두 포함(AND)" },
                        "kind": { "type": "string", "enum": ["text", "image", "audio", "video"] },
                        "since": { "type": "string", "description": "YYYY-MM-DD 또는 RFC3339. 경계 포함" },
                        "until": { "type": "string", "description": "YYYY-MM-DD 또는 RFC3339. 경계 포함" },
                        "limit": { "type": "integer", "default": 20 }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "soul_similar",
                "description": "이 관측과 비슷한 관측들. 캐시된 벡터끼리 코사인을 재므로 네트워크를 쓰지 않는다",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "observation_id": { "type": "string" },
                        "n": { "type": "integer", "default": 5 }
                    },
                    "required": ["observation_id"],
                    "additionalProperties": false
                }
            },
            {
                "name": "soul_read",
                "description": "관측 하나의 전문(JSON). 근거가 필요할 때 원본까지 내려간다",
                "inputSchema": {
                    "type": "object",
                    "properties": { "observation_id": { "type": "string" } },
                    "required": ["observation_id"],
                    "additionalProperties": false
                }
            },
            {
                "name": "soul_examples",
                "description": "한 축에서 값이 가장 높은/낮은 관측들. 그 축이 이 사람에게 실제로 무엇인지 볼 때 쓴다",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "axis": { "type": "string", "enum": AXIS_NAMES },
                        "pole": { "type": "string", "enum": ["high", "low"] },
                        "n": { "type": "integer", "default": 5 }
                    },
                    "required": ["axis", "pole"],
                    "additionalProperties": false
                }
            },
            {
                "name": "soul_readings",
                "description": "사용자 본인이 ○/× 와 함께 쓴 문장. cell=other_reason 이 가장 강한 신호다",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "layer": { "type": "string", "enum": ["sensory", "cultural"] },
                        "verdict": { "type": "string", "enum": ["yes", "no"] },
                        "cell": { "type": "string",
                                  "enum": ["read", "other_reason", "wrong_words", "unread"] },
                        "limit": { "type": "integer", "default": 20 }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "soul_context",
                "description": "이 투입에 대한 최신 문화 비평 하나 — critique·lineage·sources",
                "inputSchema": {
                    "type": "object",
                    "properties": { "ingest_id": { "type": "string" } },
                    "required": ["ingest_id"],
                    "additionalProperties": false
                }
            },
            {
                "name": "soul_timeline",
                "description": "달별 상태(drift·crystal·누적 관측 수). month 를 생략하면 전체",
                "inputSchema": {
                    "type": "object",
                    "properties": { "month": { "type": "string", "description": "YYYY-MM" } },
                    "additionalProperties": false
                }
            }
        ]
    })
}

pub fn call(ctx: &crate::server::Ctx, name: &str, args: &Value) -> anyhow::Result<Value> {
    match name {
        "soul_recall" => recall(ctx, args),
        "soul_similar" => similar(ctx, args),
        "soul_read" => read(ctx, args),
        "soul_examples" => examples(ctx, args),
        "soul_readings" => readings(ctx, args),
        "soul_context" => context(ctx, args),
        "soul_timeline" => timeline(ctx, args),
        other => bail!("알 수 없는 툴: {other} (가능: {})", NAMES.join(", ")),
    }
}

// ─────────────────────────────────────────────────────────────────── 툴 본체

/// 태그·종류·기간 필터. 임베딩이 필요 없는 유일한 진입점이다 (§19.5).
fn recall(ctx: &Ctx, args: &Value) -> anyhow::Result<Value> {
    let limit = limit_of(ctx, args, "limit", 20);
    let tags = tags_arg(args)?;
    let kind = match arg_str(args, "kind") {
        Some(s) => Some(
            Kind::parse(s)
                .ok_or_else(|| anyhow!("알 수 없는 kind: {s} (text|image|audio|video)"))?,
        ),
        None => None,
    };
    // 날짜 필터는 §12의 파생 창이 아니라 사용자 질의이므로 **양끝을 포함한다.**
    // `since`는 구간의 첫 순간, `until`은 구간의 마지막 순간이다 — 양쪽 모두
    // `2026-07-03`을 "그 날"로 읽어야 경계 포함이 실제로 성립한다.
    let since = arg_str(args, "since").map(parse_ts).transpose()?;
    let until = arg_str(args, "until").map(parse_until).transpose()?;

    // §R9 — supersede된 ingest는 여기서 이미 빠진다 (T17).
    let mut rows: Vec<&Ingest> = ctx
        .set
        .active_ingests()
        .into_iter()
        .filter(|i| kind.is_none_or(|k| i.source.kind == k))
        .filter(|i| since.is_none_or(|t| i.ts >= t))
        .filter(|i| until.as_ref().is_none_or(|u| u.covers(i.ts)))
        .filter(|i| tags.iter().all(|t| i.machine.tags.iter().any(|x| x == t)))
        .collect();
    // 최근 것이 먼저다. 동점이면 ULID 내림차순 — 순서가 실행마다 흔들리면 안 된다.
    rows.sort_by(|a, b| b.ts.cmp(&a.ts).then_with(|| b.id.cmp(&a.id)));
    Ok(summaries(ctx, rows.into_iter().take(limit)))
}

/// §19.5 — **저장된 벡터끼리** 코사인을 잰다. 새 임베딩이 필요 없다 (T33).
fn similar(ctx: &Ctx, args: &Value) -> anyhow::Result<Value> {
    let id = obs_id(args, "observation_id")?;
    let n = limit_of(ctx, args, "n", 5);
    let active = active_index(&ctx.set);
    ensure_active_ingest(ctx, &id, &active)?;

    // T36 — 캐시가 없으면 여기서 사유와 조치를 담은 에러가 난다. 크래시하지 않는다.
    let db = ctx.db_or_err()?;
    // §12.7 — 주 공간(`obs_vec`)만 본다. 비평 벡터를 섞으면 "무엇을 좋아하는가"가
    // "무엇에 관해 글이 쓰였는가"로 흐른다 (T49).
    let all = db.obs_vec_all(Space::Object)?;
    let target = all
        .iter()
        .find(|(k, _)| k.as_str() == id.as_str())
        .map(|(_, v)| v)
        .ok_or_else(|| {
            anyhow!("관측 {id} 의 임베딩이 캐시에 없습니다. `soul rebuild` 로 캐시를 채우십시오")
        })?;

    let mut scored: Vec<(f32, &Ingest)> = all
        .iter()
        .filter(|(k, _)| k.as_str() != id.as_str())
        // 활성 ingest가 아닌 벡터(재분석으로 대체된 것 등)는 아예 후보에서 뺀다 (§R9).
        .filter_map(|(k, v)| {
            active
                .get(k.as_str())
                .map(|i| (vecmath::cosine_similarity(target, v), *i))
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
    Ok(summaries(ctx, scored.into_iter().take(n).map(|(_, i)| i)))
}

/// 관측 전문. 요약이 아니라 파일에 있는 그대로다.
fn read(ctx: &Ctx, args: &Value) -> anyhow::Result<Value> {
    let id = obs_id(args, "observation_id")?;
    let obs = ctx
        .set
        .get(&id)
        .ok_or_else(|| anyhow!("관측 {id} 을 찾을 수 없습니다"))?;
    // §R9 — 대체된 ingest는 어느 툴에서도 나오지 않는다. 대신 대체본을 가리킨다.
    if obs.as_ingest().is_some() && ctx.set.superseded_ids().contains(&id) {
        let by = ctx
            .set
            .ingests()
            .into_iter()
            .find(|i| i.supersedes.as_ref() == Some(&id))
            .map(|i| i.id.to_string())
            .unwrap_or_else(|| "(알 수 없음)".to_string());
        bail!("관측 {id} 는 재분석으로 대체되었습니다 (§R9). 대체본: {by}");
    }
    Ok(serde_json::to_value(obs)?)
}

/// 한 축의 양 끝. 축 값이 실제로 무엇을 골라내는지 보여준다.
fn examples(ctx: &Ctx, args: &Value) -> anyhow::Result<Value> {
    let name = arg_str(args, "axis").ok_or_else(|| anyhow!("axis 가 필요합니다"))?;
    let axis = Axis::parse(name)
        .ok_or_else(|| anyhow!("알 수 없는 축: {name} (가능: {})", AXIS_NAMES.join(", ")))?;
    let pole = arg_str(args, "pole").ok_or_else(|| anyhow!("pole 이 필요합니다 (high|low)"))?;
    let high = match pole {
        "high" => true,
        "low" => false,
        other => bail!("pole 은 high 또는 low 입니다: {other}"),
    };
    let n = limit_of(ctx, args, "n", 5);

    let mut rows = ctx.set.active_ingests();
    rows.sort_by(|a, b| {
        let (x, y) = (a.machine.axes.get(axis), b.machine.axes.get(axis));
        if high {
            y.total_cmp(&x)
        } else {
            x.total_cmp(&y)
        }
        .then_with(|| a.id.cmp(&b.id))
    });
    Ok(summaries(ctx, rows.into_iter().take(n)))
}

/// **사용자 본인이 쓴 문장.** 기계 서술이 아니다 (§6.3).
fn readings(ctx: &Ctx, args: &Value) -> anyhow::Result<Value> {
    let limit = limit_of(ctx, args, "limit", 20);
    let layer = match arg_str(args, "layer") {
        Some(s) => Some(
            Layer::parse(s).ok_or_else(|| anyhow!("layer 는 sensory 또는 cultural 입니다: {s}"))?,
        ),
        None => None,
    };
    let verdict = match arg_str(args, "verdict") {
        Some(s) => {
            Some(Verdict::parse(s).ok_or_else(|| anyhow!("verdict 는 yes 또는 no 입니다: {s}"))?)
        }
        None => None,
    };
    let cell = match arg_str(args, "cell") {
        Some(s) => Some(Cell::parse(s).ok_or_else(|| {
            anyhow!("알 수 없는 cell: {s} (read|other_reason|wrong_words|unread)")
        })?),
        None => None,
    };

    let dead = ctx.set.superseded_ids();
    let mut rows: Vec<&Reading> = Vec::new();
    for r in ctx.set.readings() {
        if layer.is_some_and(|l| l != r.layer) {
            continue;
        }
        if verdict.is_some_and(|v| v != r.verdict) {
            continue;
        }
        // 응답이 결국 어느 ingest를 가리키는지 되짚는다. cultural은 context를 거치는
        // 2단 조인이다 (§12.6, T55c).
        let ingest = ingest_of(&ctx.set, r);
        if ingest.as_ref().is_some_and(|i| dead.contains(i)) {
            continue; // §R9
        }
        if let Some(want) = cell {
            let Some(i) = ingest.as_ref() else { continue };
            if cell_of(&ctx.set, i) != Some(want) {
                continue;
            }
        }
        rows.push(r);
    }
    rows.sort_by(|a, b| b.ts.cmp(&a.ts).then_with(|| b.id.cmp(&a.id)));

    let max = ctx.config.mcp.prose_max_chars;
    Ok(Value::Array(
        rows.into_iter()
            .take(limit)
            .map(|r| {
                json!({
                    // sensory면 ingest, cultural이면 context의 id다 (§6.3).
                    "target": r.target.as_str(),
                    "layer": r.layer.as_str(),
                    "verdict": r.verdict.as_str(),
                    // verdict=yes 면 언제나 null이다 (§6.3, T7).
                    "prose": r.prose.as_deref().map(|p| clip(p, max)),
                })
            })
            .collect(),
    ))
}

/// 최신 `context` 하나 (§12.6 — 재요청 시 이전 것은 셀 계산에서 무시된다).
fn context(ctx: &Ctx, args: &Value) -> anyhow::Result<Value> {
    let id = obs_id(args, "ingest_id")?;
    let active = active_index(&ctx.set);
    ensure_active_ingest(ctx, &id, &active)?;
    let c = ctx.set.latest_context_for(&id).ok_or_else(|| {
        anyhow!(
            "관측 {id} 에 대한 context 가 없습니다 \
             (비평 대기 중이거나 thresholds.context_enabled=false 입니다)"
        )
    })?;
    Ok(json!({
        "critique": c.critique,
        "lineage": c.lineage,
        // 실제 검색어(queries)는 내보내지 않는다 — 프라이버시 고지의 대상이다 (§D6).
        "sources": c.sources.iter().map(|s| json!({
            "url": s.url,
            "title": s.title,
            "fetched_at": s.fetched_at.to_rfc3339_millis(),
        })).collect::<Vec<_>>(),
        "grounded": c.grounded,
    }))
}

/// §12.5 `state_at`. 버킷은 달력 월이다.
fn timeline(ctx: &Ctx, args: &Value) -> anyhow::Result<Value> {
    let embeds = ctx.embeds();
    let all = state::timeline(&ctx.set, &embeds, ctx.config.local.silhouette_max_samples);
    match arg_str(args, "month") {
        None => Ok(serde_json::to_value(all)?),
        Some(m) => {
            let month =
                Month::parse(m).ok_or_else(|| anyhow!("month 는 YYYY-MM 형식입니다: {m}"))?;
            let found = all
                .into_iter()
                .find(|s| s.month == month)
                .ok_or_else(|| anyhow!("{month} 의 상태가 없습니다 (관측 기간 밖입니다)"))?;
            Ok(serde_json::to_value(found)?)
        }
    }
}

// ───────────────────────────────────────────────────────────────────── 공용

/// 관측 요약 — `{id, ts, kind, prose, tags, surprisal}`.
/// **`axes`와 임베딩은 넣지 않는다.** 필요하면 `soul_read`로 전문을 읽으면 된다.
fn summary(i: &Ingest, prose_max: usize) -> Value {
    json!({
        "id": i.id.as_str(),
        "ts": i.ts.to_rfc3339_millis(),
        "kind": i.source.kind.as_str(),
        "prose": clip(&i.machine.prose, prose_max),
        "tags": i.machine.tags,
        "surprisal": (i.surprisal * 1000.0).round() / 1000.0,
    })
}

fn summaries<'a>(ctx: &Ctx, rows: impl Iterator<Item = &'a Ingest>) -> Value {
    let max = ctx.config.mcp.prose_max_chars;
    Value::Array(rows.map(|i| summary(i, max)).collect())
}

/// 활성 ingest의 `id -> &Ingest` 색인. `active_ingests()`가 §R9의 유일한 관문이다.
fn active_index(set: &ObsSet) -> BTreeMap<&str, &Ingest> {
    set.active_ingests()
        .into_iter()
        .map(|i| (i.id.as_str(), i))
        .collect()
}

/// 주어진 ID가 **살아 있는 투입**인지 확인하고, 아니면 이유를 말한다.
fn ensure_active_ingest(
    ctx: &Ctx,
    id: &ObsId,
    active: &BTreeMap<&str, &Ingest>,
) -> anyhow::Result<()> {
    if active.contains_key(id.as_str()) {
        return Ok(());
    }
    match ctx.set.get(id) {
        None => bail!("관측 {id} 을 찾을 수 없습니다"),
        Some(o) if o.as_ingest().is_some() => {
            bail!("관측 {id} 는 재분석으로 대체되었습니다 (§R9). 대체본을 쓰십시오")
        }
        Some(o) => bail!(
            "관측 {id} 는 투입(ingest)이 아니라 {} 입니다",
            o.type_name()
        ),
    }
}

/// `reading`이 최종적으로 가리키는 ingest. cultural은 `context`를 한 번 더 거친다 (T55c).
fn ingest_of(set: &ObsSet, r: &Reading) -> Option<ObsId> {
    match r.layer {
        Layer::Sensory => set
            .get(&r.target)
            .and_then(|o| o.as_ingest())
            .map(|i| i.id.clone()),
        Layer::Cultural => set
            .get(&r.target)
            .and_then(|o| o.as_context())
            .map(|c| c.target.clone()),
    }
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
}

/// 정수 인자. 모델이 `"5"`처럼 문자열로 보내는 일이 잦아 둘 다 받는다.
fn arg_usize(args: &Value, key: &str) -> Option<usize> {
    match args.get(key)? {
        Value::Number(n) => n.as_u64().map(|v| v as usize),
        Value::String(s) => s.trim().parse::<usize>().ok(),
        _ => None,
    }
}

/// 건수는 언제나 `mcp.max_recall_limit` 이하다 (§19.9).
fn limit_of(ctx: &Ctx, args: &Value, key: &str, default: usize) -> usize {
    let cap = ctx.config.mcp.max_recall_limit.max(1);
    arg_usize(args, key).unwrap_or(default).clamp(1, cap)
}

fn obs_id(args: &Value, key: &str) -> anyhow::Result<ObsId> {
    let s = arg_str(args, key).ok_or_else(|| anyhow!("{key} 가 필요합니다"))?;
    ObsId::parse(s).map_err(|_| anyhow!("잘못된 관측 ID: {s} (26자 ULID여야 합니다)"))
}

fn tags_arg(args: &Value) -> anyhow::Result<Vec<String>> {
    match args.get("tags") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(s)) => Ok(vec![s.clone()]),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| anyhow!("tags 는 문자열 배열입니다"))
            })
            .collect(),
        Some(_) => bail!("tags 는 문자열 배열입니다"),
    }
}

/// `YYYY-MM-DD` · `YYYY-MM` · RFC3339 를 받는다. 앞의 둘은 그 구간의 첫 순간이다.
/// **하한(`since`) 전용이다.** 상한은 [`parse_until`]을 쓴다.
fn parse_ts(s: &str) -> anyhow::Result<Ts> {
    if let Some(t) = Ts::parse(s) {
        return Ok(t);
    }
    if s.len() == 10 {
        if let Some(t) = Ts::parse(&format!("{s}T00:00:00.000Z")) {
            return Ok(t);
        }
    }
    if s.len() == 7 {
        if let Some(m) = Month::parse(s) {
            return Ok(m.start());
        }
    }
    bail!("날짜 형식이 아닙니다: {s} (YYYY-MM-DD 또는 RFC3339)")
}

/// 상한 경계 (§19.4 `until`).
///
/// `until=2026-07-03` 이라고 쓴 사람의 뜻은 "그 날까지 전부"다. 하한과 같은 규칙으로
/// 그 날 00:00 을 상한으로 삼으면 **그 날에 기록된 관측이 통째로 사라진다** — 툴 설명이
/// 약속한 "경계 포함"과 반대다. 그래서 날짜·월은 구간의 끝까지 늘리고, RFC3339 로
/// 특정 순간을 지정한 경우에만 그 순간에서 끊는다.
enum Upper {
    /// 그 순간을 포함한다.
    At(Ts),
    /// 그 순간 **직전**까지 포함한다 (다음 구간의 첫 순간).
    Before(Ts),
}

impl Upper {
    fn covers(&self, t: Ts) -> bool {
        match *self {
            Upper::At(u) => t <= u,
            Upper::Before(u) => t < u,
        }
    }
}

fn parse_until(s: &str) -> anyhow::Result<Upper> {
    if let Some(t) = Ts::parse(s) {
        return Ok(Upper::At(t));
    }
    if s.len() == 10 {
        // ts 직렬화가 밀리초까지이므로 `23:59:59.999` 가 그 날의 마지막 순간이다 (§R1).
        if let Some(t) = Ts::parse(&format!("{s}T23:59:59.999Z")) {
            return Ok(Upper::At(t));
        }
    }
    if s.len() == 7 {
        // 달의 마지막 날은 달마다 다르다. 다음 달 첫 순간을 미포함 상한으로 쓴다.
        if let Some(m) = Month::parse(s) {
            return Ok(Upper::Before(m.end_exclusive()));
        }
    }
    bail!("날짜 형식이 아닙니다: {s} (YYYY-MM-DD 또는 RFC3339)")
}

/// **문자 단위**로 자른다 (§19.9 `prose_max_chars`). 0이면 자르지 않는다.
fn clip(s: &str, max: usize) -> String {
    if max == 0 || s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_match_the_declared_list() {
        let v = list();
        let names: Vec<&str> = v["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, NAMES.to_vec());
        // §19.4의 툴 수는 7이다. 늘리기 전에 기존 툴에 인자를 더하는 쪽을 검토한다.
        assert_eq!(names.len(), 7);
    }

    #[test]
    fn every_tool_has_an_object_schema() {
        for t in list()["tools"].as_array().unwrap() {
            let s = &t["inputSchema"];
            assert_eq!(s["type"], "object", "{}", t["name"]);
            assert_eq!(s["additionalProperties"], false, "{}", t["name"]);
        }
    }

    #[test]
    fn clip_counts_characters_not_bytes() {
        // 한글은 UTF-8에서 3바이트다. 바이트로 자르면 글자가 깨진다.
        assert_eq!(clip("가나다라", 2), "가나…");
        assert_eq!(clip("가나다라", 4), "가나다라");
        assert_eq!(clip("가나다라", 0), "가나다라", "0은 제한 없음이다");
    }

    #[test]
    fn parse_ts_accepts_date_month_and_rfc3339() {
        assert_eq!(
            parse_ts("2026-08-13").unwrap().to_rfc3339_millis(),
            "2026-08-13T00:00:00.000Z"
        );
        assert_eq!(
            parse_ts("2026-08").unwrap().to_rfc3339_millis(),
            "2026-08-01T00:00:00.000Z"
        );
        assert_eq!(
            parse_ts("2026-08-13T09:12:33.123Z")
                .unwrap()
                .to_rfc3339_millis(),
            "2026-08-13T09:12:33.123Z"
        );
        assert!(parse_ts("작년").is_err());
    }

    #[test]
    fn until_covers_the_whole_day_and_the_whole_month() {
        // 날짜만 준 `until` 이 그 날 00:00 에서 끊기면 그 날의 관측이 전부 사라진다.
        let noon = Ts::parse("2026-08-13T12:00:00.000Z").unwrap();
        assert!(
            parse_until("2026-08-13").unwrap().covers(noon),
            "그 날 전체를 포함한다"
        );
        assert!(
            parse_until("2026-08").unwrap().covers(noon),
            "그 달 전체를 포함한다"
        );
        assert!(!parse_until("2026-08-12").unwrap().covers(noon));
        assert!(!parse_until("2026-07").unwrap().covers(noon));

        // 달의 마지막 날은 달마다 다르다 — 2월도 6월도 끝까지 들어와야 한다.
        let feb = Ts::parse("2026-02-28T23:59:59.999Z").unwrap();
        assert!(parse_until("2026-02").unwrap().covers(feb));
        assert!(!parse_until("2026-02")
            .unwrap()
            .covers(Ts::parse("2026-03-01T00:00:00.000Z").unwrap()));

        // RFC3339 로 특정 순간을 주면 거기서 끊는다. 하루로 늘리지 않는다.
        let u = parse_until("2026-08-13T12:00:00.000Z").unwrap();
        assert!(u.covers(noon), "지정한 순간은 포함한다");
        assert!(!u.covers(Ts::parse("2026-08-13T12:00:00.001Z").unwrap()));

        assert!(parse_until("작년").is_err());
    }

    #[test]
    fn int_args_accept_strings_too() {
        let v = json!({"limit": "7", "n": 3, "bad": true});
        assert_eq!(arg_usize(&v, "limit"), Some(7));
        assert_eq!(arg_usize(&v, "n"), Some(3));
        assert_eq!(arg_usize(&v, "bad"), None);
        assert_eq!(arg_usize(&v, "없음"), None);
    }

    #[test]
    fn tags_accept_one_string_or_a_list() {
        assert_eq!(tags_arg(&json!({"tags": "실내"})).unwrap(), vec!["실내"]);
        assert_eq!(
            tags_arg(&json!({"tags": ["실내", "무인"]})).unwrap(),
            vec!["실내", "무인"]
        );
        assert!(tags_arg(&json!({})).unwrap().is_empty());
        assert!(tags_arg(&json!({"tags": [1]})).is_err());
    }
}
