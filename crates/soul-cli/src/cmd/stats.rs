//! `soul stats [--json]` (§14 · §R11).
//!
//! **§R11 — `prompt_sha256`이 바뀌는 관측 경계를 반드시 출력한다.** 프롬프트를 고치면
//! 축 값과 서술의 분포가 통째로 바뀌므로, 경계가 보이지 않으면 프롬프트 수정이
//! 가짜 드리프트로 읽힌다 (T23). `--json`이든 사람이 읽는 출력이든 항상 나온다.
//!
//! `--json`은 `derived::stats::Stats`를 **정준 JSON**으로 낸다 (§R6).
//! 이 명령은 아무것도 쓰지 않는다 — 파생 캐시를 만들지도 갱신하지도 않는다.

use anyhow::Result;
use soul_core::canon;
use soul_core::config::Config;
use soul_core::db::Db;
use soul_core::derived::{stats as core_stats, Derived};
use soul_core::obs::{Axis, ObsSet};
use soul_core::paths::Paths;
use soul_core::rebuild;
use soul_core::soulmd::{fmt_change, fmt_value, NULL_GLYPH};
use std::collections::BTreeMap;

pub fn stats(paths: &Paths, json: bool) -> Result<()> {
    // 읽기만 한다. 미스가 있어도 에러가 아니다 — 통계는 오프라인에서도 봐야 한다.
    let (derived, missing) = rebuild::replay(paths, false)?;
    let set = super::rebuild::load_set(paths)?;
    let k = cluster_k(paths, &set)?;
    let s = core_stats::build(&derived, &set, k);

    if json {
        // §R6 — 키 사전순·2칸·LF·부동소수 6자리.
        //
        // `canon::to_string`이 **이미 끝에 LF를 붙인다.** `println!`을 쓰면 빈 줄이 하나
        // 더 붙어 `soul stats --json > f.json`이 정준 형태와 바이트 동일하지 않게 된다.
        print!("{}", canon::to_string(&s)?);
        return Ok(());
    }

    if !missing.is_empty() {
        eprintln!(
            "경고: 임베딩 캐시 미스 {}건 — coherence·crystal·군집이 {NULL_GLYPH}일 수 있습니다",
            missing.len()
        );
    }
    print_human(&s);
    Ok(())
}

fn print_human(s: &core_stats::Stats) {
    print!("{}", human_report(s));
}

/// 사람이 읽는 보고서 전문.
///
/// 출력과 분리해 두는 이유는 하나다 — **§R11 경계가 실제로 찍히는지 테스트가 봐야 한다.**
/// `println!`로 흩어 두면 그 규칙을 단위 테스트로 잡을 방법이 없다.
fn human_report(s: &core_stats::Stats) -> String {
    use std::fmt::Write;

    let d: &Derived = &s.derived;
    let mut o = String::new();

    let period = match (d.t_first, d.t_ref) {
        (Some(a), Some(b)) => format!("{} … {}", a.date_string(), b.date_string()),
        _ => NULL_GLYPH.to_string(),
    };
    let t_ref = d
        .t_ref
        .map(|t| t.to_rfc3339_millis())
        .unwrap_or_else(|| NULL_GLYPH.to_string());

    let _ = writeln!(
        o,
        "관측 {}건 · 활성 ingest {}건",
        d.total_observation_count, d.observation_count
    );
    let _ = writeln!(o, "기간 {period} · 기준 T_ref {t_ref}");
    let _ = writeln!(o);
    let _ = writeln!(o, "종류     {}", counts(&s.counts_by_type));
    let _ = writeln!(o, "kind     {}", counts(&s.counts_by_kind));
    let _ = writeln!(o, "quality  {}", counts(&s.counts_by_quality));
    let _ = writeln!(o);

    let _ = writeln!(o, "축 (final = computed + offset, §12.1)");
    for a in Axis::ALL {
        let v = d.axes_final.map(|x| x.get(a));
        let off = d.axes_offset.get(a);
        let _ = writeln!(
            o,
            "  {:<10} {}  90일 {}{}",
            a.name(),
            fmt_value(v),
            fmt_change(d.axes_change[a.index()]),
            if off == 0.0 {
                String::new()
            } else {
                format!("  offset {}", fmt_change(Some(off)))
            }
        );
    }
    let _ = writeln!(o);

    let c = &d.cells;
    let _ = writeln!(
        o,
        "셀       read {} · other_reason {} · wrong_words {} · unread {} (합 {})",
        c.read,
        c.other_reason,
        c.wrong_words,
        c.unread,
        c.total()
    );
    let _ = writeln!(
        o,
        "어긋남   비율 {} · 감각 {} · 문화 {}",
        fmt_value(d.misread_ratio),
        fmt_value(d.divergence_sensory),
        fmt_value(d.divergence_cultural)
    );
    let _ = writeln!(
        o,
        "coherence 감각 {} · 문화 {}",
        coherence(&d.coherence_sensory),
        coherence(&d.coherence_cultural)
    );
    let _ = writeln!(
        o,
        "해상도   crystal {} · 군집 {}",
        fmt_value(d.crystal_now),
        s.cluster_k
            .map(|k| k.to_string())
            .unwrap_or_else(|| NULL_GLYPH.to_string())
    );
    let _ = writeln!(o, "타임라인 {}개월", d.timeline.len());
    let _ = writeln!(o);

    // §R11 — 여기가 이 명령의 존재 이유다. 비어 있어도 줄을 생략하지 않는다.
    let _ = writeln!(
        o,
        "prompt_sha256 경계 (§R11) — {}개",
        d.prompt_boundaries.len()
    );
    if d.prompt_boundaries.is_empty() {
        let _ = writeln!(o, "  {NULL_GLYPH}");
    }
    // 종류를 함께 찍는다. `ingest`는 `describe.md`, `soul_delta`는 `reflect.md`의 해시라
    // 서로 비교할 수 있는 값이 아니다 — 종류가 없으면 사용자가 두 줄을 같은 축으로 읽는다.
    for b in &d.prompt_boundaries {
        let _ = writeln!(o, "  {}  {}  {}", b.id, b.kind, b.sha256);
    }
    o
}

fn coherence(c: &Option<soul_core::derived::Coherence>) -> String {
    match c {
        None => NULL_GLYPH.to_string(),
        Some(c) => format!(
            "{} (표본 {}{})",
            fmt_value(Some(c.value)),
            c.sample,
            if c.systematic { ", 체계적" } else { "" }
        ),
    }
}

fn counts(m: &BTreeMap<String, usize>) -> String {
    if m.is_empty() {
        return NULL_GLYPH.to_string();
    }
    m.iter()
        .map(|(k, v)| format!("{k} {v}"))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// 현재 군집 수 (§12.3).
///
/// 캐시된 군집을 그대로 믿지 않는다 — `--from-scratch` 직후에는 비어 있고, 그 뒤
/// 투입이 있었으면 낡았다. 활성 ingest의 임베딩이 전부 캐시에 있으면 §R5 규칙대로
/// 다시 계산하고(고정 시드 42라 두 번 돌려도 같다, T12), 하나라도 없으면
/// 캐시된 값으로 물러난다. 둘 다 없으면 `None` → `—`로 렌더된다 (§R10).
fn cluster_k(paths: &Paths, set: &ObsSet) -> Result<Option<usize>> {
    // 계산 규칙은 `soul_core::derived::cluster_k` 하나뿐이다. MCP 도 같은 것을 쓴다 —
    // 같은 저장소에 두 답이 나오면 둘 다 못 믿는다.
    let db_path = paths.derived_db();
    if !db_path.is_file() {
        return Ok(None);
    }
    let db = Db::open(&db_path)?;
    let cfg = Config::load(&paths.config_toml())?;
    Ok(soul_core::derived::cluster_k(&db, set, &cfg)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_render_null_glyph_when_empty() {
        assert_eq!(counts(&BTreeMap::new()), NULL_GLYPH);
        let m: BTreeMap<String, usize> = [("image".to_string(), 2), ("text".to_string(), 1)].into();
        assert_eq!(counts(&m), "image 2 · text 1");
    }

    #[test]
    fn coherence_renders_null_and_sample() {
        assert_eq!(coherence(&None), NULL_GLYPH);
        let c = soul_core::derived::Coherence {
            value: 0.523,
            sample: 7,
            examples: vec![],
            systematic: true,
        };
        assert_eq!(coherence(&Some(c)), "0.52 (표본 7, 체계적)");
    }

    /// §R11 — 경계가 비어 있어도 절이 사라지지 않는다.
    #[test]
    fn empty_stats_still_mention_prompt_boundaries() {
        let s = core_stats::build(&Derived::default(), &ObsSet::default(), None);
        assert!(s.derived.prompt_boundaries.is_empty());
        // 모든 값이 null인 경로에서도 패닉 없이 돌고, 절이 남아 있어야 한다 (§R10).
        let r = human_report(&s);
        assert!(r.contains("prompt_sha256 경계 (§R11) — 0개"), "{r}");
        assert!(r.contains(&format!("\n  {NULL_GLYPH}\n")), "{r}");
    }

    // ─────────────────────────────────────────────── §R11 · T23

    fn id(n: u32) -> soul_core::ids::ObsId {
        soul_core::ids::ObsId::parse(&format!("01J8XQZK3M7P4RSTVWXYZ{n:05}")).unwrap()
    }

    fn ingest(n: u32, prompt_sha: &str) -> soul_core::obs::Observation {
        use soul_core::obs::*;
        Observation::Ingest(Ingest {
            id: id(n),
            ts: soul_core::time::Ts::parse(&format!("2026-08-{n:02}T09:12:33.123Z")).unwrap(),
            schema: soul_core::SCHEMA_VERSION,
            source: Source {
                kind: Kind::Image,
                sha256: format!("{n:064}"),
                origin: format!("file:///tmp/{n}.jpg"),
                bytes: 100,
                mime: "image/jpeg".into(),
            },
            machine: Machine {
                prose: format!("서술 {n}"),
                axes: Axes::ZERO,
                tags: vec![],
                quality: Quality::Full,
                prompt_sha256: prompt_sha.into(),
            },
            min_dist: None,
            surprisal: 0.0,
            model: ModelRef {
                provider: "openai".into(),
                id: "gpt-x".into(),
                prompt_sha256: None,
                calls: vec![],
            },
            supersedes: None,
        })
    }

    fn soul_delta(n: u32, prompt_sha: Option<&str>) -> soul_core::obs::Observation {
        use soul_core::obs::*;
        Observation::SoulDelta(SoulDelta {
            id: id(n),
            ts: soul_core::time::Ts::parse(&format!("2026-08-{n:02}T09:12:33.123Z")).unwrap(),
            schema: soul_core::SCHEMA_VERSION,
            window: Window {
                from: id(1),
                to: id(n),
            },
            blocks: std::collections::BTreeMap::new(),
            axis_delta: AxisDelta::new(),
            morphology_delta: None,
            cites: vec![],
            rationale: "제안".into(),
            model: ModelRef {
                provider: "openai".into(),
                id: "gpt-x".into(),
                prompt_sha256: prompt_sha.map(str::to_string),
                calls: vec![],
            },
        })
    }

    /// `soul stats`가 실제로 거치는 경로 그대로 만든다.
    ///
    /// **`Derived::default()`로 만들면 안 된다** — 경계를 채우는 것은 `stats::build`가
    /// 아니라 `derived::compute`다. 기본값으로 조립하면 경계가 비어 있는데도 테스트가
    /// 통과해 §R11이 무방비가 된다.
    fn stats_of(set: &ObsSet) -> core_stats::Stats {
        let embeds: BTreeMap<String, Vec<f32>> = BTreeMap::new();
        let d = soul_core::derived::compute(set, &embeds, 500);
        core_stats::build(&d, set, None)
    }

    /// T23 — `prompts/describe.md`를 고치면 그 뒤 관측의 `prompt_sha256`이 바뀌고,
    /// `soul stats`가 **그 경계를 출력한다.** 이게 없으면 프롬프트 수정이 가짜 드리프트로 읽힌다.
    #[test]
    fn prompt_boundary_is_printed_with_its_observation_id() {
        let set = ObsSet::new(vec![
            ingest(1, "aaa111"),
            ingest(2, "aaa111"),
            ingest(3, "bbb222"), // 여기서 프롬프트가 바뀌었다
        ]);
        let s = stats_of(&set);
        let boundaries = set.prompt_boundaries();
        assert!(!boundaries.is_empty(), "픽스처가 경계를 만들어야 한다");
        assert_eq!(s.derived.prompt_boundaries, boundaries);

        let r = human_report(&s);
        assert!(
            r.contains(&format!(
                "prompt_sha256 경계 (§R11) — {}개",
                boundaries.len()
            )),
            "{r}"
        );
        for b in &boundaries {
            assert!(
                r.contains(&format!("  {}  {}  {}", b.id, b.kind, b.sha256)),
                "경계 줄이 없다:\n{r}"
            );
        }
        assert!(r.contains("bbb222"), "바뀐 sha가 보여야 한다:\n{r}");
    }

    /// §R11 — `soul_delta.model.prompt_sha256`이 바뀌는 지점도 같은 목록에 나온다.
    /// 성찰 프롬프트를 고치면 제안의 어휘와 축 변화폭이 통째로 움직인다.
    #[test]
    fn soul_delta_prompt_boundaries_are_printed_too() {
        let set = ObsSet::new(vec![
            ingest(1, "aaa111"),
            soul_delta(2, Some("ref111")),
            ingest(3, "aaa111"),
            soul_delta(4, Some("ref222")), // 여기서 reflect.md 가 바뀌었다
        ]);
        let r = human_report(&stats_of(&set));
        assert!(r.contains("prompt_sha256 경계 (§R11) — 3개"), "{r}");
        assert!(r.contains("soul_delta  ref111"), "{r}");
        assert!(r.contains("soul_delta  ref222"), "{r}");
        assert!(r.contains("ingest  aaa111"), "{r}");
    }

    /// §R6 — `--json`은 정준 JSON이다. 키 사전순·2칸 들여쓰기·LF·끝에 개행 없음.
    /// 그리고 `prompt_boundaries`가 그 안에 들어 있다 (§R11은 두 출력 모두에 적용된다).
    #[test]
    fn json_output_is_canonical_and_carries_the_boundaries() {
        let set = ObsSet::new(vec![ingest(1, "aaa111"), ingest(2, "bbb222")]);
        let j = canon::to_string(&stats_of(&set)).unwrap();

        let v: serde_json::Value = serde_json::from_str(&j).unwrap();
        // `Stats`는 `derived`를 flatten 한다 — 경계는 최상위에 온다.
        assert!(
            !v["prompt_boundaries"].as_array().unwrap().is_empty(),
            "{j}"
        );
        assert!(!j.contains('\r'), "LF만 쓴다 (§R6)");
        // 끝 LF는 `canon::to_string`이 붙인다. `stats`가 `print!`를 쓰는 이유이며,
        // `println!`로 되돌리면 빈 줄이 하나 더 생겨 정준 형태와 어긋난다.
        assert!(j.ends_with("}\n"), "정준 JSON은 LF 하나로 끝난다");
        assert!(!j.ends_with("\n\n"), "빈 줄이 붙으면 안 된다");
    }

    // ─────────────────────────────────────────────── 쓰지 않는다

    /// 이 명령은 아무것도 쓰지 않는다. 특히 `derived.sqlite`를 **만들지 않는다** —
    /// 읽기 경로가 빈 캐시 파일을 만들면 그다음 `rebuild --offline`이 미스를 놓친다.
    #[test]
    fn stats_creates_no_files() {
        let root = std::env::temp_dir()
            .join("tasty-soul-cli-stats")
            .join(soul_core::ids::new_id().to_string());
        let paths = Paths::at(&root);
        paths.ensure_dirs().unwrap();
        let o = ingest(1, "aaa111");
        let f = paths.observation_file(o.ts(), o.id());
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(&f, o.to_canonical_json().unwrap()).unwrap();

        stats(&paths, false).unwrap();
        stats(&paths, true).unwrap();

        assert!(!paths.derived_db().exists(), "파생 캐시를 만들지 않는다");
        assert!(!paths.soul_md().exists(), "SOUL.md를 쓰지 않는다");
        let _ = std::fs::remove_dir_all(&root);
    }
}
