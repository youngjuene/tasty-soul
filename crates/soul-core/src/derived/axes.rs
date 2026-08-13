//! 축 값 (§12.1) 과 90일 변화 (§12.2).
//!
//! ## §12.1 — 유일한 계산 지점
//!
//! ```text
//! w_i         = quality_weight(quality_i) × 0.5^((T_ref − ts_i) / 30일)
//! computed[a] = Σ(w_i × axes_i[a]) / Σ(w_i)              # Σw = 0 이면 0.5
//! offset[a]   = Σ(axis_delta[a]) over 모든 soul_delta      # 없으면 0
//! final[a]    = clamp(computed[a] + offset[a], 0, 1)
//! ```
//!
//! **관측 평균과 에이전트 델타는 이 식으로만 합성한다. 다른 어디에서도 축 값을 계산하지 않는다.**
//! 입력은 `ObsSet::active_ingests()` — supersede된 것은 이미 빠져 있다 (§R9, T17).
//!
//! ## §12.2 — 90일 변화
//!
//! ```text
//! recent[a] = quality 가중 평균 over ts ∈ (T_ref−90d, T_ref]
//! prior[a]  = quality 가중 평균 over ts ∈ (T_ref−180d, T_ref−90d]
//! change[a] = recent[a] − prior[a]        # 어느 창이든 3건 미만이면 null
//! ```
//!
//! **시간 감쇠는 적용하지 않는다.** 창 자체가 시간을 자른다.
//! **`change`에는 `offset`을 더하지 않는다.** `final`은 "현재 판단", `change`는 "관측된 이동"이다.
//! 이는 의도된 비대칭이다 (§12.2).

use crate::obs::{Axes, Axis, Ingest, ObsSet};
use crate::time::Ts;

/// 반감기 30일 지수 감쇠 가중 (§12.1).
pub fn decay_weight(t_ref: Ts, ts: Ts) -> f64 {
    0.5f64.powf(t_ref.days_since(ts) / 30.0)
}

/// quality × decay 가중 평균. `Σw = 0`이면 모든 축이 `0.5`다.
pub fn weighted_mean(ingests: &[&Ingest], t_ref: Ts, apply_decay: bool) -> Axes {
    let mut acc = [0.0f64; 8];
    let mut sum_w = 0.0f64;

    for ing in ingests {
        // quality_weight는 §6.2의 표(1.0 / 0.6 / 0.2)를 그대로 쓴다 (T57).
        // 이 수를 여기에 다시 적지 않는다.
        let decay = if apply_decay {
            decay_weight(t_ref, ing.ts)
        } else {
            1.0
        };
        let w = ing.machine.quality.weight() * decay;
        if !w.is_finite() {
            continue;
        }
        sum_w += w;
        let a = ing.machine.axes.to_array();
        for (i, v) in a.iter().enumerate() {
            acc[i] += w * v;
        }
    }

    // §12.1 — Σw = 0 이면 모든 축이 0.5다. 관측이 없거나 감쇠가 언더플로한 경우.
    if sum_w <= 0.0 {
        return Axes::from_array([0.5; 8]);
    }
    for v in acc.iter_mut() {
        *v /= sum_w;
    }
    Axes::from_array(acc)
}

/// 모든 `soul_delta`의 `axis_delta` 합 (§12.1의 `offset`).
pub fn offset(set: &ObsSet) -> Axes {
    let mut out = Axes::ZERO;
    for d in set.soul_deltas() {
        for (key, delta) in &d.axis_delta {
            // 알 수 없는 축 이름은 기록 시점(`Observation::validate`)에 이미 거부된다.
            // 그래도 조용히 건너뛴다 — 파생 계산은 절대 실패하지 않는다.
            if let Some(ax) = Axis::parse(key) {
                out.set(ax, out.get(ax) + delta);
            }
        }
    }
    out
}

/// `clamp(computed + offset, 0, 1)` (T15).
pub fn finalize(computed: Axes, offset: Axes) -> Axes {
    let c = computed.to_array();
    let o = offset.to_array();
    let mut out = [0.0f64; 8];
    for i in 0..8 {
        out[i] = (c[i] + o[i]).clamp(0.0, 1.0);
    }
    Axes::from_array(out)
}

/// §12.2. 축별로 `None`이 될 수 있다. 반환 순서는 `Axis::ALL`이다.
pub fn change_90d(set: &ObsSet, t_ref: Ts) -> [Option<f64>; 8] {
    let d90 = t_ref.minus_days(90);
    let d180 = t_ref.minus_days(180);

    // §R9 — supersede된 ingest는 §12의 모든 계산에서 빠진다 (T17).
    let active = set.active_ingests();

    // 창 경계는 §R1의 관례를 따른다: 아래는 열림, 위는 닫힘.
    let recent: Vec<&Ingest> = active
        .iter()
        .copied()
        .filter(|i| i.ts > d90 && i.ts <= t_ref)
        .collect();
    let prior: Vec<&Ingest> = active
        .iter()
        .copied()
        .filter(|i| i.ts > d180 && i.ts <= d90)
        .collect();

    // 어느 창이든 3건 미만이면 전 축이 null이다.
    if recent.len() < 3 || prior.len() < 3 {
        return [None; 8];
    }

    // 시간 감쇠는 적용하지 않는다 — 창 자체가 시간을 자른다.
    let r = weighted_mean(&recent, t_ref, false).to_array();
    let p = weighted_mean(&prior, t_ref, false).to_array();

    // offset을 더하지 않는다. `final`은 "현재 판단", `change`는 "관측된 이동"이다 (§12.2).
    let mut out = [None; 8];
    for i in 0..8 {
        out[i] = Some(r[i] - p[i]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ObsId;
    use crate::obs::{Kind, Machine, ModelRef, Observation, Quality, SoulDelta, Source, Window};
    use std::collections::BTreeMap;

    const T_REF: &str = "2026-08-13T00:00:00.000Z";

    fn ts(s: &str) -> Ts {
        Ts::parse(s).expect("테스트의 고정 타임스탬프는 항상 유효하다")
    }

    /// 26자 Crockford base32. 뒤 16자에 순번을 넣어 사전순 = 순번순이 되게 한다.
    fn oid(n: u64) -> ObsId {
        ObsId::parse(&format!("01J0000000{n:016X}")).expect("고정 형식의 유효한 ULID")
    }

    /// 8축 전부 같은 값을 갖는 ingest. 가중 평균 검산이 쉬워진다.
    fn ingest(n: u64, at: &str, q: Quality, v: f64) -> Ingest {
        Ingest {
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
                axes: Axes::from_array([v; 8]),
                tags: vec![],
                quality: q,
                prompt_sha256: "p0".into(),
            },
            min_dist: None,
            surprisal: 1.0,
            model: ModelRef {
                provider: "test".into(),
                id: "m".into(),
                prompt_sha256: None,
                calls: vec![],
            },
            supersedes: None,
        }
    }

    fn soul_delta(n: u64, at: &str, axis: &str, v: f64) -> Observation {
        let mut axis_delta = BTreeMap::new();
        axis_delta.insert(axis.to_string(), v);
        Observation::SoulDelta(SoulDelta {
            id: oid(n),
            ts: ts(at),
            schema: crate::SCHEMA_VERSION,
            window: Window {
                from: oid(0),
                to: oid(n),
            },
            blocks: BTreeMap::new(),
            axis_delta,
            morphology_delta: None,
            cites: vec![],
            rationale: "테스트".into(),
            model: ModelRef {
                provider: "test".into(),
                id: "m".into(),
                prompt_sha256: None,
                calls: vec![],
            },
        })
    }

    fn set_of(v: Vec<Observation>) -> ObsSet {
        ObsSet::new(v)
    }

    // ─────────────────────────────────────────────────────────── §12.1

    #[test]
    fn decay_is_exactly_half_at_thirty_days() {
        let t = ts(T_REF);
        assert!((decay_weight(t, t) - 1.0).abs() < 1e-12, "0일이면 1.0");
        assert!(
            (decay_weight(t, t.minus_days(30)) - 0.5).abs() < 1e-12,
            "반감기 30일"
        );
        assert!((decay_weight(t, t.minus_days(60)) - 0.25).abs() < 1e-12);
        assert!((decay_weight(t, t.minus_days(90)) - 0.125).abs() < 1e-12);
    }

    #[test]
    fn quality_weights_are_full_partial_minimal() {
        // T57 — 1.0 / 0.6 / 0.2. 감쇠는 끄고 quality만 본다.
        let t = ts(T_REF);
        let a = ingest(1, T_REF, Quality::Full, 1.0);
        let b = ingest(2, T_REF, Quality::Partial, 0.5);
        let c = ingest(3, T_REF, Quality::Minimal, 0.0);
        let m = weighted_mean(&[&a, &b, &c], t, false);
        // (1.0×1.0 + 0.6×0.5 + 0.2×0.0) / 1.8
        let expect = 1.3 / 1.8;
        for v in m.to_array() {
            assert!((v - expect).abs() < 1e-12, "{v} != {expect}");
        }
    }

    #[test]
    fn empty_input_is_half_on_every_axis() {
        // Σw = 0 → 전 축 0.5.
        let m = weighted_mean(&[], ts(T_REF), true);
        assert_eq!(m.to_array(), [0.5; 8]);
    }

    #[test]
    fn decay_halves_a_thirty_day_old_observation() {
        let t = ts(T_REF);
        let new = ingest(1, T_REF, Quality::Full, 1.0);
        let old = ingest(2, "2026-07-14T00:00:00.000Z", Quality::Full, 0.0);
        let m = weighted_mean(&[&new, &old], t, true);
        // (1.0×1.0 + 0.5×0.0) / 1.5
        let expect = 1.0 / 1.5;
        for v in m.to_array() {
            assert!((v - expect).abs() < 1e-12, "{v} != {expect}");
        }
        // 감쇠를 끄면 단순 평균 0.5가 된다.
        let flat = weighted_mean(&[&new, &old], t, false);
        assert!((flat.chroma - 0.5).abs() < 1e-12);
    }

    #[test]
    fn offset_sums_every_soul_delta() {
        let s = set_of(vec![
            soul_delta(1, T_REF, "chroma", 0.10),
            soul_delta(2, T_REF, "chroma", -0.04),
            soul_delta(3, T_REF, "tempo", 0.25),
        ]);
        let o = offset(&s);
        assert!((o.chroma - 0.06).abs() < 1e-12);
        assert!((o.tempo - 0.25).abs() < 1e-12);
        assert_eq!(o.grain, 0.0, "델타가 없는 축은 0이다");
    }

    #[test]
    fn offset_is_zero_without_soul_delta() {
        assert_eq!(offset(&set_of(vec![])), Axes::ZERO);
    }

    #[test]
    fn final_is_clamped_to_unit_range() {
        // T15 — axis_delta 누적 후에도 final은 [0,1]을 벗어나지 않는다.
        let computed = Axes::from_array([0.9, 0.1, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5]);
        let off = Axes::from_array([0.5, -0.5, 0.2, -0.2, 0.0, 0.0, 0.0, 0.0]);
        let f = finalize(computed, off);
        assert_eq!(f.chroma, 1.0, "0.9 + 0.5 → 1.0으로 클램프");
        assert_eq!(f.luminance, 0.0, "0.1 − 0.5 → 0.0으로 클램프");
        assert!((f.density - 0.7).abs() < 1e-12);
        assert!((f.grain - 0.3).abs() < 1e-12);
        assert!(f.in_unit_range());
    }

    // ─────────────────────────────────────────────────────────── §12.2

    /// 최근 3건(0.8) + 이전 3건(0.3). T_ref−90d = 2026-05-15, T_ref−180d = 2026-02-14.
    fn two_window_set() -> Vec<Observation> {
        vec![
            Observation::Ingest(ingest(11, "2026-06-01T00:00:00.000Z", Quality::Full, 0.8)),
            Observation::Ingest(ingest(12, "2026-07-01T00:00:00.000Z", Quality::Full, 0.8)),
            Observation::Ingest(ingest(13, "2026-08-01T00:00:00.000Z", Quality::Full, 0.8)),
            Observation::Ingest(ingest(1, "2026-03-01T00:00:00.000Z", Quality::Full, 0.3)),
            Observation::Ingest(ingest(2, "2026-04-01T00:00:00.000Z", Quality::Full, 0.3)),
            Observation::Ingest(ingest(3, "2026-05-01T00:00:00.000Z", Quality::Full, 0.3)),
        ]
    }

    #[test]
    fn change_is_recent_minus_prior() {
        let s = set_of(two_window_set());
        let c = change_90d(&s, ts(T_REF));
        for v in c {
            let v = v.expect("두 창 모두 3건이므로 값이 있어야 한다");
            assert!((v - 0.5).abs() < 1e-12, "0.8 − 0.3 = 0.5, got {v}");
        }
    }

    #[test]
    fn change_has_no_decay_and_no_offset() {
        // 감쇠가 걸렸다면 최근 창 안의 오래된 건이 덜 반영되어 0.5가 아니게 된다.
        // offset이 더해졌다면 0.5 + 0.2 = 0.7이 된다 (§12.2의 의도된 비대칭).
        let mut obs = two_window_set();
        obs.push(soul_delta(90, T_REF, "chroma", 0.2));
        let c = change_90d(&set_of(obs), ts(T_REF));
        assert!((c[0].expect("chroma") - 0.5).abs() < 1e-12);
    }

    #[test]
    fn change_is_null_when_a_window_has_fewer_than_three() {
        let mut obs = two_window_set();
        obs.pop(); // 이전 창을 2건으로 줄인다
        let c = change_90d(&set_of(obs), ts(T_REF));
        assert!(c.iter().all(|v| v.is_none()), "이전 창 2건 → 전 축 null");

        let only_recent = set_of(two_window_set().into_iter().take(3).collect());
        assert!(change_90d(&only_recent, ts(T_REF))
            .iter()
            .all(|v| v.is_none()));
    }

    #[test]
    fn window_boundary_belongs_to_the_prior_window() {
        // (T_ref−90d, T_ref] 는 아래가 열려 있다. 정확히 T_ref−90d 인 건은 이전 창이다.
        let mut obs = two_window_set();
        obs.remove(0); // 최근 창을 2건으로
        obs.push(Observation::Ingest(ingest(
            20,
            "2026-05-15T00:00:00.000Z",
            Quality::Full,
            0.3,
        )));
        let s = set_of(obs);
        assert!(
            change_90d(&s, ts(T_REF)).iter().all(|v| v.is_none()),
            "경계 건이 최근 창에 들어갔다면 3건이 되어 값이 나왔을 것이다"
        );
    }

    #[test]
    fn superseded_ingest_is_excluded_from_change() {
        // T17 — supersede된 건이 섞이면 최근 평균이 0.9가 아니라 0.675로 기운다.
        let mut obs = two_window_set();
        // 최근 창의 세 건을 0.9로 바꾸고, 값이 0.0인 낡은 건과 그것을 대체하는 건을 넣는다.
        obs.retain(|o| o.ts() < ts("2026-05-15T00:00:00.000Z"));
        for (n, at) in [
            (11, "2026-06-01T00:00:00.000Z"),
            (12, "2026-07-01T00:00:00.000Z"),
        ] {
            obs.push(Observation::Ingest(ingest(n, at, Quality::Full, 0.9)));
        }
        obs.push(Observation::Ingest(ingest(
            13,
            "2026-06-15T00:00:00.000Z",
            Quality::Full,
            0.0,
        )));
        let mut newer = ingest(14, "2026-08-01T00:00:00.000Z", Quality::Full, 0.9);
        newer.supersedes = Some(oid(13));
        obs.push(Observation::Ingest(newer));

        let c = change_90d(&set_of(obs), ts(T_REF));
        let v = c[0].expect("최근 3건(활성) + 이전 3건");
        assert!(
            (v - 0.6).abs() < 1e-12,
            "0.9 − 0.3 = 0.6 이어야 한다 (supersede된 0.0이 빠짐), got {v}"
        );
    }
}
