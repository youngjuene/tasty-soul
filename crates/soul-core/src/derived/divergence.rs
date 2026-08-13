//! 어긋남 (§12.6) — 2×2 셀이 **주 지표**, 실수 지표는 보조다.
//!
//! ## 셀 — 2단 조인 (T55c)
//!
//! 문화 판정은 `ingest`가 아니라 **`context`를 대상으로** 하므로 2단 조인이다.
//! 한 단으로 구현하면 셀이 영원히 `null`로 남는다.
//!
//! ```text
//! cell(ingest):
//!   s_read = ingest를 target으로 하는 layer=sensory reading 중 ULID 최대
//!   ctx    = ingest를 target으로 하는 context 중 ULID 최대        # 재요청 시 최신만
//!   c_read = ctx를 target으로 하는 layer=cultural reading 중 ULID 최대
//!
//!   s_read 또는 c_read가 없으면 → null
//!   그 외 → (s_read.verdict, c_read.verdict)
//! ```
//!
//! `ctx`가 여러 개일 때 **이전 `context`에 달린 응답은 셀 계산에서 무시한다.**
//! 다만 `divergence` 집계에는 포함된다 — 그 시점의 어긋남은 실제로 있었던 사실이다 (T55b).
//!
//! ## 보조 실수 지표
//!
//! ```text
//! divergence_sensory  = mean(reading.divergence)
//!                       over layer = "sensory",  ts ∈ (T_ref−30d, T_ref], divergence ≠ null
//! divergence_cultural = 같은 식, layer = "cultural"      # 각각 표본 0건이면 null
//! ```
//!
//! ## 체계적 어긋남 — 창은 **전체 기간**이다
//!
//! ```text
//! R = prose ≠ null 인 모든 reading (기간 제한 없음). **layer별로 따로 계산한다**
//! 각 r ∈ R 에 대해:
//!   v_r = normalize(embed(r.prose)) − normalize(embed(target.machine.prose))
//! mean_v    = mean(v_r)
//! coherence = ‖mean_v‖ / mean(‖v_r‖)      # 0 = 무작위, 1 = 완전 일관. |R| < 2 이면 null
//! ```
//!
//! `coherence ≥ 0.4` 이고 `|R| ≥ 5` 이면 체계적 어긋남이 존재한다고 본다.
//! 대표 사례는 `v_r`이 `mean_v`와 코사인 유사도가 가장 높은 `reading` 3개의 **`target` ID**.
//!
//! `layer=cultural`의 대상 텍스트는 `context.critique`이며 그 임베딩은
//! **별도 공간**에 있다 (§12.7). `EmbeddingLookup`은 텍스트로 조회하므로 호출자가
//! 올바른 텍스트를 넘기기만 하면 되지만, 두 공간을 섞어 군집에 넣지 말 것 (T49).

use super::{Cell, CellCounts, Coherence, EmbeddingLookup};
use crate::ids::ObsId;
use crate::obs::{Layer, ObsSet, Verdict};
use crate::time::Ts;
use crate::vecmath;

/// 한 ingest의 셀. 두 층 응답이 모두 있어야 정해진다 (T55).
pub fn cell_of(set: &ObsSet, ingest: &ObsId) -> Option<Cell> {
    // 1단: 감각 응답은 ingest를 직접 가리킨다.
    let s_read = set.latest_reading_for(ingest, Layer::Sensory)?;
    // 2단: 문화 응답은 ingest가 아니라 context를 가리킨다 (T55c).
    //      context가 여럿이면 최신 것만 본다 — 이전 context에 달린 응답은 셀에서 무시한다 (T55b).
    let ctx = set.latest_context_for(ingest)?;
    let c_read = set.latest_reading_for(&ctx.id, Layer::Cultural)?;

    Some(match (s_read.verdict, c_read.verdict) {
        (Verdict::Yes, Verdict::Yes) => Cell::Read,
        (Verdict::Yes, Verdict::No) => Cell::OtherReason,
        (Verdict::No, Verdict::Yes) => Cell::WrongWords,
        (Verdict::No, Verdict::No) => Cell::Unread,
    })
}

/// 활성 ingest 전체의 셀 분포. `window`가 있으면 그 기간의 ingest만 센다.
pub fn cell_counts(set: &ObsSet, since: Option<Ts>) -> CellCounts {
    let mut counts = CellCounts::default();
    // §R9 — supersede된 ingest는 세지 않는다 (T17).
    for i in set.active_ingests() {
        // 창은 §R1의 관례대로 아래가 열려 있다: (since, T_ref].
        if let Some(from) = since {
            if i.ts <= from {
                continue;
            }
        }
        if let Some(cell) = cell_of(set, &i.id) {
            counts.bump(cell);
        }
    }
    counts
}

/// `read`가 아닌 셀의 비율 (§8.2.1 헤더의 `어긋남`). 셀이 하나도 없으면 `None`.
pub fn misread_ratio(counts: &CellCounts) -> Option<f64> {
    if counts.total() == 0 {
        return None;
    }
    Some((counts.total() - counts.read) as f64 / counts.total() as f64)
}

/// 최근 30일 layer별 divergence 평균. 표본 0건이면 `None`.
pub fn divergence_mean(set: &ObsSet, layer: Layer, t_ref: Ts) -> Option<f64> {
    let from = t_ref.minus_days(30);
    // supersede된 ingest를 가리키는 reading도 그대로 쓴다 (§R9).
    // 이전 context에 달린 응답도 포함한다 — 그 시점의 어긋남은 실제로 있었던 사실이다 (T55b).
    let mut sum = 0.0f64;
    let mut n = 0usize;
    for r in set.readings() {
        if r.layer != layer || r.ts <= from || r.ts > t_ref {
            continue;
        }
        if let Some(d) = r.divergence {
            sum += d;
            n += 1;
        }
    }
    if n == 0 {
        return None;
    }
    Some(sum / n as f64)
}

/// 체계적 어긋남. **layer별로 따로 부른다.**
pub fn coherence(set: &ObsSet, layer: Layer, embeds: &dyn EmbeddingLookup) -> Option<Coherence> {
    // (reading.id, reading.target, v_r). reading.id는 동점 정렬을 결정론으로 만드는 데만 쓴다.
    let mut rows: Vec<(&ObsId, &ObsId, Vec<f32>)> = Vec::new();
    let mut dim: Option<usize> = None;

    // 창은 전체 기간이다 — 정정은 드물어 30일 창으로는 표본이 모이지 않는다.
    for r in set.readings() {
        if r.layer != layer {
            continue;
        }
        let Some(prose) = r.prose.as_deref() else {
            continue;
        };
        // 대상 텍스트: sensory는 ingest의 machine.prose, cultural은 context의 critique.
        // 두 임베딩은 서로 다른 공간에 있으므로 layer를 섞지 않는다 (§12.7, T49).
        let target_text = match layer {
            Layer::Sensory => set
                .get(&r.target)
                .and_then(|o| o.as_ingest())
                .map(|i| i.machine.prose.as_str()),
            Layer::Cultural => set
                .get(&r.target)
                .and_then(|o| o.as_context())
                .map(|c| c.critique.as_str()),
        };
        let Some(target_text) = target_text else {
            continue;
        };
        // 임베딩이 없으면 v_r을 만들 수 없다. 조용히 건너뛴다 — 파생 계산은 실패하지 않는다.
        let (Some(pv), Some(tv)) = (embeds.get(prose), embeds.get(target_text)) else {
            continue;
        };
        if pv.is_empty() || pv.len() != tv.len() {
            continue;
        }
        // 차원이 섞이면 평균을 낼 수 없다. 처음 본 차원으로 고정한다.
        match dim {
            None => dim = Some(pv.len()),
            Some(d) if d != pv.len() => continue,
            _ => {}
        }

        let pn = vecmath::normalized(&pv);
        let tn = vecmath::normalized(&tv);
        let v: Vec<f32> = pn.iter().zip(tn.iter()).map(|(a, b)| a - b).collect();
        rows.push((&r.id, &r.target, v));
    }

    // |R| < 2 이면 null (T19).
    if rows.len() < 2 {
        return None;
    }

    let vectors: Vec<Vec<f32>> = rows.iter().map(|(_, _, v)| v.clone()).collect();
    let mean_v = vecmath::mean(&vectors)?;
    let mean_len = rows
        .iter()
        .map(|(_, _, v)| vecmath::norm(v) as f64)
        .sum::<f64>()
        / rows.len() as f64;

    // v_r이 전부 영벡터면 방향 자체가 없다. 0(무작위)으로 둔다.
    // 삼각부등식으로 ‖mean_v‖ ≤ mean(‖v_r‖) 이므로 값은 [0,1]이다.
    let value = if mean_len > 0.0 {
        (vecmath::norm(&mean_v) as f64 / mean_len).clamp(0.0, 1.0)
    } else {
        0.0
    };

    // 대표 사례: v_r이 mean_v와 코사인 유사도가 가장 높은 reading 3개의 target ID.
    let mut ranked: Vec<(f64, &ObsId, &ObsId)> = rows
        .iter()
        .map(|(rid, tid, v)| (vecmath::cosine_similarity(v, &mean_v) as f64, *rid, *tid))
        .collect();
    ranked.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.cmp(b.1))
    });
    let examples: Vec<ObsId> = ranked
        .iter()
        .take(3)
        .map(|(_, _, t)| (*t).clone())
        .collect();

    Some(Coherence {
        value,
        sample: rows.len(),
        examples,
        systematic: value >= 0.4 && rows.len() >= 5,
    })
}

/// §6.3 — `reading.divergence` 계산식. 투입/응답 시점에 부른다.
/// `sensory` → `cosine_distance(embed(target.machine.prose), embed(prose))`
/// `cultural` → `cosine_distance(embed(target.critique), embed(prose))`
pub fn reading_divergence(target_text_embed: &[f32], prose_embed: &[f32]) -> f64 {
    crate::vecmath::cosine_distance(target_text_embed, prose_embed) as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obs::{
        Axes, ContextObs, Ingest, Kind, Machine, ModelRef, Observation, Quality, Reading, Source,
    };
    use std::collections::BTreeMap;

    const T_REF: &str = "2026-08-13T00:00:00.000Z";

    fn ts(s: &str) -> Ts {
        Ts::parse(s).expect("테스트의 고정 타임스탬프는 항상 유효하다")
    }

    /// 26자 Crockford base32. 뒤 16자에 순번을 넣어 사전순 = 순번순이 되게 한다.
    fn oid(n: u64) -> ObsId {
        ObsId::parse(&format!("01J0000000{n:016X}")).expect("고정 형식의 유효한 ULID")
    }

    fn model() -> ModelRef {
        ModelRef {
            provider: "test".into(),
            id: "m".into(),
            prompt_sha256: None,
            calls: vec![],
        }
    }

    fn ingest(n: u64, at: &str) -> Observation {
        Observation::Ingest(Ingest {
            id: oid(n),
            ts: ts(at),
            schema: crate::SCHEMA_VERSION,
            source: Source {
                kind: Kind::Text,
                sha256: format!("sha{n}"),
                origin: format!("clipboard:{n}"),
                bytes: 1,
                mime: "text/plain".into(),
            },
            machine: Machine {
                prose: format!("산문 {n}"),
                axes: Axes::from_array([0.5; 8]),
                tags: vec![],
                quality: Quality::Full,
                prompt_sha256: "p0".into(),
            },
            min_dist: None,
            surprisal: 1.0,
            model: model(),
            supersedes: None,
        })
    }

    fn superseding_ingest(n: u64, at: &str, old: u64) -> Observation {
        let mut o = ingest(n, at);
        if let Observation::Ingest(i) = &mut o {
            i.supersedes = Some(oid(old));
        }
        o
    }

    fn context(n: u64, at: &str, target: u64) -> Observation {
        Observation::Context(ContextObs {
            id: oid(n),
            ts: ts(at),
            schema: crate::SCHEMA_VERSION,
            target: oid(target),
            critique: format!("비평 {n}"),
            lineage: vec![],
            queries: vec![],
            sources: vec![],
            grounded: false,
            model: model(),
        })
    }

    fn reading(
        n: u64,
        at: &str,
        layer: Layer,
        target: u64,
        verdict: Verdict,
        divergence: Option<f64>,
    ) -> Observation {
        // §6.3 — verdict=yes 면 prose는 반드시 null이고, prose 없이 divergence를 둘 수 없다.
        let prose = if verdict == Verdict::No {
            Some(format!("정정 {n}"))
        } else {
            None
        };
        Observation::Reading(Reading {
            id: oid(n),
            ts: ts(at),
            schema: crate::SCHEMA_VERSION,
            layer,
            target: oid(target),
            verdict,
            prose,
            divergence: if verdict == Verdict::No {
                divergence
            } else {
                None
            },
        })
    }

    fn set_of(v: Vec<Observation>) -> ObsSet {
        ObsSet::new(v)
    }

    /// ingest 1건 + 감각 응답 + context + 문화 응답 = 셀 하나가 성립하는 최소 구성.
    fn full_chain(ingest_n: u64, at: &str, s: Verdict, c: Verdict) -> Vec<Observation> {
        vec![
            ingest(ingest_n, at),
            reading(ingest_n + 100, at, Layer::Sensory, ingest_n, s, Some(0.5)),
            context(ingest_n + 200, at, ingest_n),
            reading(
                ingest_n + 300,
                at,
                Layer::Cultural,
                ingest_n + 200,
                c,
                Some(0.5),
            ),
        ]
    }

    // ───────────────────────────────────────────────────── 2×2 셀 (§12.6)

    #[test]
    fn cell_needs_both_layers() {
        // T55 — 한쪽 응답만 있으면 null.
        let only_sensory = set_of(vec![
            ingest(1, T_REF),
            reading(2, T_REF, Layer::Sensory, 1, Verdict::Yes, None),
        ]);
        assert_eq!(cell_of(&only_sensory, &oid(1)), None);

        let only_cultural = set_of(vec![
            ingest(1, T_REF),
            context(3, T_REF, 1),
            reading(4, T_REF, Layer::Cultural, 3, Verdict::Yes, None),
        ]);
        assert_eq!(cell_of(&only_cultural, &oid(1)), None);

        // context는 있으나 문화 응답이 아직 없는 경우도 null이다.
        let no_answer = set_of(vec![
            ingest(1, T_REF),
            reading(2, T_REF, Layer::Sensory, 1, Verdict::Yes, None),
            context(3, T_REF, 1),
        ]);
        assert_eq!(cell_of(&no_answer, &oid(1)), None);
    }

    #[test]
    fn two_stage_join_resolves_the_cell() {
        // T55c — 문화 응답이 context를 target으로 해도 셀이 성립해야 한다.
        for (s, c, expect) in [
            (Verdict::Yes, Verdict::Yes, Cell::Read),
            (Verdict::Yes, Verdict::No, Cell::OtherReason),
            (Verdict::No, Verdict::Yes, Cell::WrongWords),
            (Verdict::No, Verdict::No, Cell::Unread),
        ] {
            let set = set_of(full_chain(1, T_REF, s, c));
            assert_eq!(
                cell_of(&set, &oid(1)),
                Some(expect),
                "({s:?}, {c:?}) → {expect:?}"
            );
        }
    }

    #[test]
    fn older_context_answer_is_ignored_by_cell_but_kept_in_divergence() {
        // T55b — context가 2개면 셀은 최신 것에 달린 응답만 본다.
        //        이전 응답은 관측으로 남아 divergence 집계에는 포함된다.
        let set = set_of(vec![
            ingest(1, T_REF),
            reading(10, T_REF, Layer::Sensory, 1, Verdict::Yes, None),
            context(30, "2026-08-01T00:00:00.000Z", 1),
            context(31, "2026-08-05T00:00:00.000Z", 1), // 재요청 — ULID가 더 크다
            reading(
                40,
                "2026-08-02T00:00:00.000Z",
                Layer::Cultural,
                30,
                Verdict::No,
                Some(0.9),
            ),
            reading(
                41,
                "2026-08-06T00:00:00.000Z",
                Layer::Cultural,
                31,
                Verdict::Yes,
                None,
            ),
        ]);

        assert_eq!(
            cell_of(&set, &oid(1)),
            Some(Cell::Read),
            "최신 context(31)의 yes만 반영되어야 한다"
        );
        let d =
            divergence_mean(&set, Layer::Cultural, ts(T_REF)).expect("이전 응답이 표본에 남는다");
        assert!(
            (d - 0.9).abs() < 1e-12,
            "이전 context에 달린 응답도 divergence 집계에는 남는다, got {d}"
        );
    }

    #[test]
    fn cell_counts_respects_window_and_supersedes() {
        let mut obs = full_chain(1, "2026-08-10T00:00:00.000Z", Verdict::Yes, Verdict::No);
        obs.extend(full_chain(
            2,
            "2026-01-01T00:00:00.000Z",
            Verdict::No,
            Verdict::No,
        ));
        // T17 — supersede된 ingest는 세지 않는다.
        obs.extend(full_chain(
            3,
            "2026-08-11T00:00:00.000Z",
            Verdict::Yes,
            Verdict::Yes,
        ));
        obs.push(superseding_ingest(9, "2026-08-12T00:00:00.000Z", 3));
        let set = set_of(obs);

        let all = cell_counts(&set, None);
        assert_eq!(all.other_reason, 1);
        assert_eq!(all.unread, 1);
        assert_eq!(all.read, 0, "supersede된 ingest의 셀은 빠진다");
        assert_eq!(all.total(), 2);

        let recent = cell_counts(&set, Some(ts(T_REF).minus_days(30)));
        assert_eq!(recent.total(), 1, "30일 창 밖의 1월 건은 빠진다");
        assert_eq!(recent.other_reason, 1);
    }

    #[test]
    fn misread_ratio_is_non_read_share() {
        assert_eq!(misread_ratio(&CellCounts::default()), None);
        let c = CellCounts {
            read: 1,
            other_reason: 1,
            wrong_words: 1,
            unread: 1,
        };
        assert_eq!(misread_ratio(&c), Some(0.75));
    }

    // ─────────────────────────────────────────────── 보조 실수 지표 (§12.6)

    #[test]
    fn divergence_mean_is_layer_scoped_and_windowed() {
        let set = set_of(vec![
            ingest(1, T_REF),
            context(2, T_REF, 1),
            reading(
                10,
                "2026-08-01T00:00:00.000Z",
                Layer::Sensory,
                1,
                Verdict::No,
                Some(0.4),
            ),
            reading(
                11,
                "2026-08-10T00:00:00.000Z",
                Layer::Sensory,
                1,
                Verdict::No,
                Some(0.6),
            ),
            // 창 밖 (T_ref−30d 이전) — 빠져야 한다.
            reading(
                12,
                "2026-06-01T00:00:00.000Z",
                Layer::Sensory,
                1,
                Verdict::No,
                Some(1.9),
            ),
            // divergence가 없는 응답 — 표본에 들어가지 않는다.
            reading(13, T_REF, Layer::Sensory, 1, Verdict::Yes, None),
            reading(14, T_REF, Layer::Cultural, 2, Verdict::No, Some(0.2)),
        ]);

        let s = divergence_mean(&set, Layer::Sensory, ts(T_REF)).expect("표본 2건");
        assert!((s - 0.5).abs() < 1e-12, "(0.4 + 0.6) / 2, got {s}");
        let c = divergence_mean(&set, Layer::Cultural, ts(T_REF)).expect("표본 1건");
        assert!((c - 0.2).abs() < 1e-12, "got {c}");

        let empty = set_of(vec![ingest(1, T_REF)]);
        assert_eq!(divergence_mean(&empty, Layer::Sensory, ts(T_REF)), None);
    }

    // ─────────────────────────────────────────── 체계적 어긋남 (coherence)

    fn embeds(pairs: &[(&str, [f32; 2])]) -> BTreeMap<String, Vec<f32>> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_vec()))
            .collect()
    }

    /// ingest n + 감각 정정 응답 (n+100). `flip`이면 v_r의 방향이 뒤집힌다.
    fn sensory_correction(n: u64, flip: bool) -> (Vec<Observation>, Vec<(String, Vec<f32>)>) {
        let obs = vec![
            ingest(n, T_REF),
            reading(n + 100, T_REF, Layer::Sensory, n, Verdict::No, Some(0.5)),
        ];
        let (t, p) = if flip {
            ([0.0f32, 1.0], [1.0f32, 0.0])
        } else {
            ([1.0f32, 0.0], [0.0f32, 1.0])
        };
        let e = vec![
            (format!("산문 {n}"), t.to_vec()),
            (format!("정정 {}", n + 100), p.to_vec()),
        ];
        (obs, e)
    }

    fn build(specs: &[(u64, bool)]) -> (ObsSet, BTreeMap<String, Vec<f32>>) {
        let mut obs = Vec::new();
        let mut emb = BTreeMap::new();
        for (n, flip) in specs {
            let (o, e) = sensory_correction(*n, *flip);
            obs.extend(o);
            emb.extend(e);
        }
        (set_of(obs), emb)
    }

    #[test]
    fn coherence_is_none_below_two_samples() {
        // T19 — 관측이 적어 |R| < 2 이면 null이다.
        let (set, emb) = build(&[(1, false)]);
        assert_eq!(coherence(&set, Layer::Sensory, &emb), None);

        // prose가 없는 응답(yes)만 있으면 표본이 0이다.
        let only_yes = set_of(vec![
            ingest(1, T_REF),
            reading(101, T_REF, Layer::Sensory, 1, Verdict::Yes, None),
            ingest(2, T_REF),
            reading(102, T_REF, Layer::Sensory, 2, Verdict::Yes, None),
        ]);
        assert_eq!(coherence(&only_yes, Layer::Sensory, &emb), None);
    }

    #[test]
    fn coherence_is_one_when_every_direction_agrees() {
        let (set, emb) = build(&[(1, false), (2, false), (3, false)]);
        let c = coherence(&set, Layer::Sensory, &emb).expect("|R| = 3");
        assert!(
            (c.value - 1.0).abs() < 1e-6,
            "완전 일관 = 1.0, got {}",
            c.value
        );
        assert_eq!(c.sample, 3);
        assert!(!c.systematic, "|R| = 3 < 5 이면 체계적이라 부르지 않는다");
        assert_eq!(c.examples, vec![oid(1), oid(2), oid(3)]);
    }

    #[test]
    fn coherence_is_zero_when_directions_cancel() {
        let (set, emb) = build(&[(1, false), (2, true)]);
        let c = coherence(&set, Layer::Sensory, &emb).expect("|R| = 2");
        assert!(c.value.abs() < 1e-6, "무작위 = 0.0, got {}", c.value);
        assert!(!c.systematic);
    }

    #[test]
    fn systematic_needs_value_and_sample() {
        // 5건 중 4건이 같은 방향 → ‖mean_v‖/mean‖v_r‖ = 0.6.
        let (set, emb) = build(&[(1, false), (2, false), (3, false), (4, false), (5, true)]);
        let c = coherence(&set, Layer::Sensory, &emb).expect("|R| = 5");
        assert!((c.value - 0.6).abs() < 1e-6, "got {}", c.value);
        assert_eq!(c.sample, 5);
        assert!(c.systematic, "0.6 ≥ 0.4 이고 |R| = 5 ≥ 5");
        // 대표 사례는 mean_v와 가장 가까운 3건의 target ID (동점은 reading ULID 오름차순).
        assert_eq!(c.examples, vec![oid(1), oid(2), oid(3)]);
        assert!(!c.examples.contains(&oid(5)), "반대 방향은 대표가 아니다");
    }

    #[test]
    fn cultural_coherence_uses_the_context_critique() {
        // 대상 텍스트가 context.critique이어야 한다. ingest.machine.prose를 보면 표본이 0이 된다.
        let set = set_of(vec![
            ingest(1, T_REF),
            context(20, T_REF, 1),
            reading(30, T_REF, Layer::Cultural, 20, Verdict::No, Some(0.5)),
            ingest(2, T_REF),
            context(21, T_REF, 2),
            reading(31, T_REF, Layer::Cultural, 21, Verdict::No, Some(0.5)),
        ]);
        let emb = embeds(&[
            ("비평 20", [1.0, 0.0]),
            ("정정 30", [0.0, 1.0]),
            ("비평 21", [1.0, 0.0]),
            ("정정 31", [0.0, 1.0]),
        ]);
        let c = coherence(&set, Layer::Cultural, &emb).expect("|R| = 2");
        assert!((c.value - 1.0).abs() < 1e-6);
        assert_eq!(c.examples, vec![oid(20), oid(21)], "target은 context ID다");

        // 감각 층은 표본이 없으므로 None이다 — 두 공간을 섞지 않는다 (§12.7).
        assert_eq!(coherence(&set, Layer::Sensory, &emb), None);
    }

    #[test]
    fn readings_without_embeddings_are_skipped() {
        let (set, mut emb) = build(&[(1, false), (2, false), (3, false)]);
        emb.remove("정정 103");
        let c = coherence(&set, Layer::Sensory, &emb).expect("남은 2건");
        assert_eq!(c.sample, 2, "임베딩이 없는 응답은 표본에서 빠진다");
    }

    #[test]
    fn coherence_keeps_readings_on_superseded_targets() {
        // §R9 — 대상이 supersede되어도 그 시점의 어긋남은 사실이므로 divergence 계열에 남는다.
        let (set0, emb) = build(&[(1, false), (2, false)]);
        let mut obs = set0.as_slice().to_vec();
        obs.push(superseding_ingest(9, T_REF, 1));
        let c = coherence(&set_of(obs), Layer::Sensory, &emb).expect("|R| = 2");
        assert_eq!(c.sample, 2);
    }
}
