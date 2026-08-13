//! 리소스 (§19.3) — 읽기 전용 문서.
//!
//! | URI | 내용 |
//! |---|---|
//! | `soul://md` | `SOUL.md` **원문 그대로**. `soul:human` 블록 포함 (§D4, T30·T31) |
//! | `soul://axes` | §7 축 정의 8개 + 현재 `final[a]` 값 + 90일 변화 |
//! | `soul://divergence` | 2×2 셀 개수와 층별 `coherence`, 대표 사례 ID |
//! | `soul://stats` | 관측 수, 기간, 현재 달 `crystal`, 군집 수 |
//!
//! **`soul://md`는 가공하지 않는다.** 마커 주석까지 포함해 파일 내용을 그대로 반환한다 (T31).

use crate::server::Ctx;
use serde_json::{json, Value};
use soul_core::derived::{Cell, CellCounts, Coherence, Derived};
use soul_core::obs::Axis;

pub const URI_MD: &str = "soul://md";
pub const URI_AXES: &str = "soul://axes";
pub const URI_DIVERGENCE: &str = "soul://divergence";
pub const URI_STATS: &str = "soul://stats";

/// 서버가 아는 URI 전부. `resources/read`의 유효성 검사가 이것을 쓴다.
pub const URIS: [&str; 4] = [URI_MD, URI_AXES, URI_DIVERGENCE, URI_STATS];

/// §7의 축 정의 표. **명세의 문구를 그대로 옮긴다** — 순서는 `Axis::ALL`과 같다.
const AXIS_POLES: [(&str, &str); 8] = [
    ("무채색에 가까움", "채도가 높고 색이 강함"),
    ("어둡고 그늘짐", "밝고 빛이 많음"),
    ("비어 있고 여백이 많음", "요소가 빽빽하게 들어참"),
    ("매끄럽고 깨끗함", "거칠고 노이즈·질감이 두드러짐"),
    ("멈춰 있고 느림", "빠르고 급함"),
    ("평면적이고 가까움", "깊고 멀고 트여 있음"),
    ("불안하고 서늘함", "안온하고 따뜻함"),
    ("은은하고 조용함", "강렬하고 압도적임"),
];

/// §12.6 셀의 뜻. 셀 이름만으로는 `other_reason`이 왜 중요한지 알 수 없다.
const CELL_LEGEND: [(&str, &str); 4] = [
    ("read", "기계가 이 사람을 읽었다"),
    (
        "other_reason",
        "보는 것은 같으나 끌리는 이유가 다르다 — 이 시스템이 찾으려는 것",
    ),
    ("wrong_words", "서술은 빗나갔어도 무엇이 중요한지는 통한다"),
    ("unread", "아직 못 잡았다"),
];

pub fn list() -> Value {
    json!({
        "resources": [
            {
                "uri": URI_MD,
                "name": "SOUL.md",
                "description": "취향 문서 원문. 마커 주석과 soul:human 블록을 포함한다",
                "mimeType": "text/markdown",
            },
            {
                "uri": URI_AXES,
                "name": "축",
                "description": "8축 정의와 현재 값, 90일 변화",
                "mimeType": "application/json",
            },
            {
                "uri": URI_DIVERGENCE,
                "name": "어긋남",
                "description": "2×2 셀 개수, 층별 coherence, 대표 사례 ID",
                "mimeType": "application/json",
            },
            {
                "uri": URI_STATS,
                "name": "통계",
                "description": "관측 수와 기간, 현재 달 crystal, 군집 수",
                "mimeType": "application/json",
            },
        ]
    })
}

/// `paths`만 받는 계약(§19.3). 관측 로그를 여기서 한 번 읽는다.
///
/// 서버 루프는 이미 로드된 `Ctx`를 가지고 있으므로 [`read_with`]를 부른다 —
/// 리소스를 읽을 때마다 관측 수천 건을 다시 역직렬화하지 않기 위해서다.
pub fn read(paths: &soul_core::paths::Paths, uri: &str) -> anyhow::Result<Value> {
    read_with(&Ctx::load(paths.clone())?, uri)
}

pub fn read_with(ctx: &Ctx, uri: &str) -> anyhow::Result<Value> {
    match uri {
        URI_MD => {
            let text = read_soul_md(&ctx.paths)?;
            Ok(contents(uri, "text/markdown", text))
        }
        URI_AXES => Ok(contents(uri, "application/json", pretty(&axes(ctx))?)),
        URI_DIVERGENCE => Ok(contents(uri, "application/json", pretty(&divergence(ctx))?)),
        URI_STATS => Ok(contents(uri, "application/json", pretty(&stats(ctx))?)),
        other => anyhow::bail!("알 수 없는 리소스: {other}"),
    }
}

/// T31 — **파일 바이트 그대로.** 블록 조립도, 마커 제거도, 요약도 하지 않는다.
/// 그 결과 `soul:human` 블록이 자연히 포함된다 (§D4, T30).
fn read_soul_md(paths: &soul_core::paths::Paths) -> anyhow::Result<String> {
    let path = paths.soul_md();
    let bytes = std::fs::read(&path)
        .map_err(|e| anyhow::anyhow!("SOUL.md 를 읽을 수 없습니다 ({}): {e}", path.display()))?;
    String::from_utf8(bytes)
        .map_err(|_| anyhow::anyhow!("SOUL.md 가 UTF-8이 아닙니다: {}", path.display()))
}

fn contents(uri: &str, mime: &str, text: String) -> Value {
    json!({ "contents": [ { "uri": uri, "mimeType": mime, "text": text } ] })
}

/// 리소스는 사람이 열어 볼 수도 있으므로 들여쓴 JSON으로 싣는다.
/// (툴 결과는 매 턴 컨텍스트에 쌓이므로 압축 JSON이다.)
fn pretty(v: &Value) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(v)?)
}

/// §7 + §12.1 + §12.2.
fn axes(ctx: &Ctx) -> Value {
    let d = ctx.derived();
    let items: Vec<Value> = Axis::ALL
        .iter()
        .map(|a| {
            let (low, high) = AXIS_POLES[a.index()];
            json!({
                "axis": a.name(),
                "low": low,
                "high": high,
                // 표시값은 final = clamp(computed + offset, 0, 1) 이다 (§12.1).
                "value": d.axes_final.map(|x| round(x.get(*a), 2)),
                // §12.2 — offset을 더하지 않는다. 의도된 비대칭이다.
                "change_90d": d.axes_change[a.index()].map(|v| round(v, 2)),
            })
        })
        .collect();
    json!({
        "axes": items,
        "scale": "모든 축은 [0,1]. value 는 관측 가중 평균에 승인된 보정을 더한 값, change_90d 는 보정을 뺀 90일 변화",
    })
}

/// §12.6 — 셀이 주 지표, 실수 지표는 보조다.
fn divergence(ctx: &Ctx) -> Value {
    let d = ctx.derived();
    json!({
        "cells": cells(&d.cells),
        "cells_recent30": cells(&d.cells_recent30),
        "misread_ratio": d.misread_ratio.map(|v| round(v, 3)),
        "divergence_sensory": d.divergence_sensory.map(|v| round(v, 3)),
        "divergence_cultural": d.divergence_cultural.map(|v| round(v, 3)),
        "coherence": {
            "sensory": coherence(d.coherence_sensory.as_ref()),
            "cultural": coherence(d.coherence_cultural.as_ref()),
        },
        "legend": CELL_LEGEND.iter().map(|(k, v)| (k.to_string(), json!(v))).collect::<serde_json::Map<_, _>>(),
    })
}

fn cells(c: &CellCounts) -> Value {
    let mut m = serde_json::Map::new();
    for cell in Cell::ALL {
        m.insert(cell.as_str().to_string(), json!(c.get(cell)));
    }
    m.insert("total".to_string(), json!(c.total()));
    Value::Object(m)
}

/// 대표 사례는 `reading`이 아니라 그 **`target` ID**다 (§12.6). 그대로 `soul_read`에 넣는다.
fn coherence(c: Option<&Coherence>) -> Value {
    match c {
        None => Value::Null,
        Some(c) => json!({
            "value": round(c.value, 3),
            "sample": c.sample,
            "systematic": c.systematic,
            "examples": c.examples.iter().map(|i| i.as_str()).collect::<Vec<_>>(),
        }),
    }
}

/// §19.3 — 관측 수, 기간, 현재 달 `crystal`, 군집 수.
fn stats(ctx: &Ctx) -> Value {
    let d: Derived = ctx.derived();
    // 군집 수는 캐시된 것을 그대로 읽는다. 여기서 k-means를 다시 돌리지 않는다 (§12.3).
    let cluster_k = ctx
        .db
        .as_ref()
        .and_then(|db| db.cluster_get().ok().flatten())
        .map(|(_, c)| c.k);
    let s = soul_core::derived::stats::build(&d, &ctx.set, cluster_k);

    json!({
        // §R9 — supersede된 ingest는 세지 않는다.
        "observations": d.observation_count,
        "observations_total": d.total_observation_count,
        "counts_by_type": s.counts_by_type,
        "counts_by_kind": s.counts_by_kind,
        "period": {
            "first": d.t_first.map(|t| t.to_rfc3339_millis()),
            "last": d.t_ref.map(|t| t.to_rfc3339_millis()),
            "days": match (d.t_first, d.t_ref) {
                (Some(a), Some(b)) => Some(round(b.days_since(a), 1)),
                _ => None,
            },
        },
        "crystal_now": d.crystal_now.map(|v| round(v, 3)),
        "cluster_k": cluster_k,
    })
}

/// 소수 자리를 줄인다. 토큰이 곧 비용이고, 0.30000000000000004 는 아무에게도 쓸모없다.
fn round(v: f64, digits: u32) -> f64 {
    let f = 10f64.powi(digits as i32);
    (v * f).round() / f
}

#[cfg(test)]
mod tests {
    use super::*;
    use soul_core::obs::AXIS_NAMES;

    fn ctx_at(root: &std::path::Path) -> Ctx {
        let paths = soul_core::paths::Paths::at(root);
        paths.ensure_dirs().expect("디렉토리");
        Ctx::load(paths).expect("컨텍스트")
    }

    fn tmp(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "tasty-soul-mcp-res-{tag}-{}-{}",
            std::process::id(),
            soul_core::ids::new_id()
        ))
    }

    #[test]
    fn list_declares_exactly_the_four_uris() {
        let v = list();
        let uris: Vec<&str> = v["resources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["uri"].as_str().unwrap())
            .collect();
        assert_eq!(uris, URIS.to_vec());
        // 전부 mimeType이 있어야 클라이언트가 렌더를 고를 수 있다.
        assert!(v["resources"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["mimeType"].is_string()));
    }

    #[test]
    fn axis_poles_follow_the_spec_table_order() {
        // §7 표의 행 순서 = Axis::ALL = SOUL.md 표의 행 순서 (§8.2.1).
        assert_eq!(AXIS_POLES.len(), AXIS_NAMES.len());
        let root = tmp("axes");
        let v = axes(&ctx_at(&root));
        let names: Vec<&str> = v["axes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["axis"].as_str().unwrap())
            .collect();
        assert_eq!(names, AXIS_NAMES.to_vec());
        // 관측이 없으면 §R10대로 null이다. 0으로 대체하지 않는다.
        assert!(v["axes"][0]["value"].is_null());
        assert!(v["axes"][0]["change_90d"].is_null());
        assert_eq!(v["axes"][0]["low"], AXIS_POLES[0].0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn axis_poles_match_the_shipped_prompt_table() {
        // §7의 표는 프롬프트에도 그대로 들어간다 (§R11). 두 곳이 갈라지면
        // 모델이 채점한 축과 에이전트가 읽는 축 정의가 다른 뜻이 된다.
        let Some(dir) = soul_core::paths::prompts_dir() else {
            return; // 배포본에 prompts/가 없으면 검사할 것이 없다.
        };
        let text = std::fs::read_to_string(dir.join("describe.md")).expect("describe.md");
        for (i, (low, high)) in AXIS_POLES.iter().enumerate() {
            let row = format!("| `{}` | {low} | {high} |", AXIS_NAMES[i]);
            assert!(text.contains(&row), "describe.md 에 없는 축 정의: {row}");
        }
    }

    #[test]
    fn divergence_has_all_four_cells_and_the_legend() {
        let root = tmp("div");
        let v = divergence(&ctx_at(&root));
        for c in Cell::ALL {
            assert_eq!(v["cells"][c.as_str()], 0);
            assert!(v["legend"][c.as_str()].is_string());
        }
        assert_eq!(v["cells"]["total"], 0);
        // 셀이 하나도 없으면 비율은 null이다 (§R10).
        assert!(v["misread_ratio"].is_null());
        assert!(v["coherence"]["sensory"].is_null());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stats_on_an_empty_archive_is_zeros_and_nulls() {
        let root = tmp("stats");
        let v = stats(&ctx_at(&root));
        assert_eq!(v["observations"], 0);
        assert_eq!(v["observations_total"], 0);
        assert!(v["period"]["first"].is_null());
        assert!(v["crystal_now"].is_null());
        assert!(v["cluster_k"].is_null());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_soul_md_is_a_clear_error_not_a_panic() {
        let root = tmp("nomd");
        let c = ctx_at(&root);
        let e = read_with(&c, URI_MD).unwrap_err().to_string();
        assert!(e.contains("SOUL.md"), "{e}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn round_trims_float_noise() {
        assert_eq!(round(0.1 + 0.2, 2), 0.3);
        assert_eq!(round(1.0 / 3.0, 3), 0.333);
    }
}
