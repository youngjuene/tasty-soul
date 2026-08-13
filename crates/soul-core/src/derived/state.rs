//! 시점별 상태 `state_at(month)` (§12.5).
//!
//! **버킷은 달력 월이다.** `T_ref` 기준 상대 오프셋(30일 단위)을 쓰지 않는다.
//! 상대 오프셋을 쓰면 관측이 하나만 추가돼도 `T_ref`가 밀리면서 모든 창 경계가
//! 이동하고 전체 캐시가 무효화된다 (T22·T40).
//!
//! ```text
//! M = 최초 관측이 속한 달부터 T_ref가 속한 달까지의 UTC 월 목록 (오름차순)
//!
//! state_at(month):
//!   S    = ingest with ts ≤ month 마지막 순간          # 누적
//!   W    = ingest with ts ∈ month
//!   Wp   = ingest with ts ∈ month 직전 달
//!
//!   centroid(X) = normalize(mean(e_i for i ∈ X))       # |X| < 3 이면 null
//!   drift       = cosine_distance(centroid(W), centroid(Wp))
//!
//!   C       = cluster(sample(S))                        # §12.5.1
//!   crystal = C의 실루엣 계수 평균 (코사인 거리)
//!
//!   반환: {month, drift, crystal, n: |S|}
//! ```
//!
//! `cluster()`는 표본이 아니라 **전체 `S`**에 대해 돌린다. 실루엣만 표본을 쓴다 (§12.5.1).
//! 새 관측은 언제나 현재 달에만 들어가므로 캐시 무효화는 "관측이 추가된 달 하나만"이다.

use super::{cluster, EmbeddingLookup, MonthState};
use crate::obs::{Ingest, ObsSet};
use crate::time::Month;
use crate::vecmath;
use std::collections::{BTreeMap, BTreeSet};

pub fn state_at(
    set: &ObsSet,
    month: Month,
    embeds: &dyn EmbeddingLookup,
    silhouette_max_samples: usize,
) -> MonthState {
    // §R9 — 모든 순회는 active_ingests()를 거친다. supersede된 것은 여기서 이미 빠져 있다 (T17).
    // 반환 순서는 ULID 오름차순이며, 아래 필터는 그 순서를 유지한다 (§12.3의 입력 조건).
    let active = set.active_ingests();
    let prev = month.prev();
    let end = month.end_exclusive(); // 다음 달 첫 순간(미포함) = "이 달 마지막 순간" 이하

    // S = 누적, W = 이 달, Wp = 직전 달.
    let s: Vec<&Ingest> = active.iter().copied().filter(|i| i.ts < end).collect();
    let w: Vec<&Ingest> = active
        .iter()
        .copied()
        .filter(|i| month.contains(i.ts))
        .collect();
    let wp: Vec<&Ingest> = active
        .iter()
        .copied()
        .filter(|i| prev.contains(i.ts))
        .collect();

    // n은 임베딩 유무와 무관한 **누적 ingest 수**다.
    let n = s.len();

    // drift = cosine_distance(centroid(W), centroid(Wp)). 어느 쪽이든 null이면 null.
    let drift = match (
        centroid_of(&embeddings_of(&w, embeds)),
        centroid_of(&embeddings_of(&wp, embeds)),
    ) {
        (Some(a), Some(b)) => Some(vecmath::cosine_distance(&a, &b) as f64),
        _ => None,
    };

    let crystal = crystal_of(&embeddings_of(&s, embeds), silhouette_max_samples);

    MonthState {
        month,
        drift,
        crystal,
        n,
    }
}

/// `e_i` — `machine.prose`의 **L2 정규화된** 임베딩 (§12 기호 정의).
///
/// 조회에 실패한 항목은 빠진다. `context.critique`와 `layer=cultural`의 `reading.prose`는
/// 애초에 이 조회기로 나오지 않으므로 군집에 섞이지 않는다 (§12.7, T49).
fn embeddings_of(ingests: &[&Ingest], embeds: &dyn EmbeddingLookup) -> Vec<Vec<f32>> {
    ingests
        .iter()
        .filter_map(|i| embeds.get(&i.machine.prose))
        .filter(|v| !v.is_empty())
        .map(|v| vecmath::normalized(&v))
        .collect()
}

/// `centroid(X) = normalize(mean(e_i for i ∈ X))`. **`|X| < 3` 이면 `None`** (§12.5).
///
/// 개수는 실제로 평균에 들어가는 벡터 수로 센다 — 임베딩이 없는 항목은 centroid에
/// 아무것도 기여하지 않으므로 표본으로 세지 않는다.
fn centroid_of(vectors: &[Vec<f32>]) -> Option<Vec<f32>> {
    if vectors.len() < 3 {
        return None;
    }
    vecmath::centroid(vectors)
}

/// `C = cluster(전체 S)` → 실루엣만 `sample(S)`에 대해 (§12.5·§12.5.1).
///
/// k-means는 O(n)이라 전수로 돌리고, O(n²)인 실루엣만 stride 표본으로 줄인다.
/// 표본 점의 군집 배정은 **전체 S의 배정에서 같은 인덱스를 골라** 쓴다 — 표본만
/// 다시 군집화하면 매달 다른 분할이 나와 crystal의 시계열이 의미를 잃는다.
fn crystal_of(s_vectors: &[Vec<f32>], max_samples: usize) -> Option<f64> {
    // 군집 문턱(`n < 4` → null)은 §12.3의 `cluster()` 한 곳에만 있다. 여기서 다시 적지 않는다.
    // 군집이 없으면 실루엣도 없다.
    let c = cluster::cluster(s_vectors)?;
    if c.assignment.len() != s_vectors.len() {
        // 방어: 배정 길이가 어긋나면 표본과 짝지을 수 없다.
        return None;
    }

    // 표본은 **인덱스**에 대해 뽑는다. 점과 그 배정을 같은 인덱스로 함께 골라야 한다.
    // stride 고정이라 재실행해도 같은 표본이다 (T41).
    let idx: Vec<usize> = (0..s_vectors.len()).collect();
    let picked = cluster::stride_sample(&idx, max_samples);

    let sample: Vec<Vec<f32>> = picked.iter().map(|&i| s_vectors[i].clone()).collect();
    let raw: Vec<usize> = picked.iter().map(|&i| c.assignment[i]).collect();

    // 표본에 살아남은 군집만 0..k로 다시 번호를 매긴다. 실루엣은 분할만 보므로
    // 값은 그대로이고, 표본에서 비어버린 군집이 사라져 계산이 안전해진다.
    let labels: BTreeSet<usize> = raw.iter().copied().collect();
    if labels.len() < 2 {
        // 표본에서 군집이 1개로 줄면 실루엣은 정의되지 않는다.
        return None;
    }
    let remap: BTreeMap<usize, usize> = labels
        .iter()
        .enumerate()
        .map(|(new, &old)| (old, new))
        .collect();
    let assignment: Vec<usize> = raw.iter().map(|o| remap[o]).collect();

    cluster::silhouette(&sample, &assignment, labels.len())
}

/// `M` — 최초 관측이 속한 달부터 `T_ref`가 속한 달까지 (오름차순).
pub fn months(set: &ObsSet) -> Vec<Month> {
    match (set.t_first(), set.t_ref()) {
        (Some(a), Some(b)) => a.month().range_through(b.month()),
        _ => Vec::new(),
    }
}

pub fn timeline(
    set: &ObsSet,
    embeds: &dyn EmbeddingLookup,
    silhouette_max_samples: usize,
) -> Vec<MonthState> {
    months(set)
        .into_iter()
        .map(|m| state_at(set, m, embeds, silhouette_max_samples))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ObsId;
    use crate::obs::{Axes, Kind, Machine, ModelRef, Observation, Quality, Source};
    use crate::time::Ts;
    use std::collections::BTreeMap;

    fn ts(s: &str) -> Ts {
        Ts::parse(s).expect("테스트 타임스탬프")
    }

    /// prose 텍스트가 곧 임베딩 조회 키다.
    fn ingest(at: &str, prose: &str, supersedes: Option<ObsId>) -> Observation {
        Observation::Ingest(crate::obs::Ingest {
            id: crate::ids::new_id(),
            ts: ts(at),
            schema: crate::SCHEMA_VERSION,
            source: Source {
                kind: Kind::Image,
                sha256: "0".into(),
                origin: "file:///x.jpg".into(),
                bytes: 1,
                mime: "image/jpeg".into(),
            },
            machine: Machine {
                prose: prose.into(),
                axes: Axes::from_array([0.5; 8]),
                tags: vec![],
                quality: Quality::Full,
                prompt_sha256: "p1".into(),
            },
            min_dist: None,
            surprisal: 1.0,
            model: ModelRef {
                provider: "test".into(),
                id: "m".into(),
                prompt_sha256: None,
                calls: vec![],
            },
            supersedes,
        })
    }

    fn id_of(o: &Observation) -> ObsId {
        o.id().clone()
    }

    /// 2차원 임베딩 하나. 각도로 방향을 준다.
    fn vec_at(deg: f32) -> Vec<f32> {
        let r = deg.to_radians();
        vec![r.cos(), r.sin()]
    }

    fn embeds(pairs: &[(&str, f32)]) -> BTreeMap<String, Vec<f32>> {
        pairs
            .iter()
            .map(|(k, d)| ((*k).to_string(), vec_at(*d)))
            .collect()
    }

    #[test]
    fn empty_set_does_not_crash() {
        let set = ObsSet::default();
        let e: BTreeMap<String, Vec<f32>> = BTreeMap::new();
        assert!(months(&set).is_empty());
        assert!(timeline(&set, &e, 500).is_empty());

        let s = state_at(&set, Month::new(2026, 8), &e, 500);
        assert_eq!(s.n, 0);
        assert_eq!(s.drift, None);
        assert_eq!(s.crystal, None);
    }

    #[test]
    fn drift_is_none_when_a_window_has_fewer_than_three() {
        // 직전 달 1건, 이 달 2건 → 양쪽 다 |X| < 3 이므로 drift는 null이다.
        let set = ObsSet::new(vec![
            ingest("2026-06-20T00:00:00.000Z", "a", None),
            ingest("2026-07-02T00:00:00.000Z", "b", None),
            ingest("2026-07-09T00:00:00.000Z", "c", None),
        ]);
        let e = embeds(&[("a", 0.0), ("b", 10.0), ("c", 20.0)]);

        let s = state_at(&set, Month::new(2026, 7), &e, 500);
        assert_eq!(s.drift, None);
        assert_eq!(s.n, 3, "n은 이 달 말까지의 누적 ingest 수다");
        assert_eq!(s.crystal, None, "n < 4 이면 군집이 없다 (§12.3)");
    }

    #[test]
    fn month_bucket_is_calendar_and_ignores_later_observations() {
        // T22·T40 — 지난 달의 상태는 새 관측이 붙어도 달라지지 않는다.
        // 달력 월이 아니라 T_ref 상대 오프셋을 쓰면 이 성질이 깨진다.
        let base = vec![
            ingest("2026-06-01T00:00:00.000Z", "a", None),
            ingest("2026-06-11T00:00:00.000Z", "b", None),
            ingest("2026-06-21T00:00:00.000Z", "c", None),
        ];
        let mut grown = base.clone();
        grown.push(ingest("2026-07-01T00:00:00.000Z", "d", None));

        let e = embeds(&[("a", 0.0), ("b", 10.0), ("c", 20.0), ("d", 90.0)]);
        let june = Month::new(2026, 6);

        let before = state_at(&ObsSet::new(base), june, &e, 500);
        let after = state_at(&ObsSet::new(grown), june, &e, 500);
        assert_eq!(before, after);
        assert_eq!(after.n, 3, "7월 관측은 6월의 누적에 들어가지 않는다");
    }

    #[test]
    fn state_at_is_stable_across_calls() {
        // T22 — 같은 입력이면 몇 번 불러도 같은 값이다 (실행 시각에 의존하지 않는다).
        let set = ObsSet::new(vec![
            ingest("2026-07-01T00:00:00.000Z", "a", None),
            ingest("2026-07-02T00:00:00.000Z", "b", None),
            ingest("2026-07-03T00:00:00.000Z", "c", None),
        ]);
        let e = embeds(&[("a", 0.0), ("b", 10.0), ("c", 20.0)]);
        let first = state_at(&set, Month::new(2026, 7), &e, 500);
        let second = state_at(&set, Month::new(2026, 7), &e, 500);
        assert_eq!(first, second);
    }

    #[test]
    fn superseded_ingests_are_excluded_from_n() {
        // T17 — 재분석된 원본은 누적 수에서 빠진다. 빠지지 않으면 이중 계상이다.
        let old = ingest("2026-06-01T00:00:00.000Z", "a", None);
        let old_id = id_of(&old);
        let set = ObsSet::new(vec![
            old,
            ingest("2026-07-01T00:00:00.000Z", "a2", Some(old_id)),
            ingest("2026-07-02T00:00:00.000Z", "c", None),
        ]);
        let e = embeds(&[("a", 0.0), ("a2", 5.0), ("c", 20.0)]);

        assert_eq!(state_at(&set, Month::new(2026, 6), &e, 500).n, 0);
        assert_eq!(state_at(&set, Month::new(2026, 7), &e, 500).n, 2);
    }

    #[test]
    fn timeline_covers_every_month_from_first_to_t_ref() {
        let set = ObsSet::new(vec![
            ingest("2025-11-20T00:00:00.000Z", "a", None),
            ingest("2026-01-05T00:00:00.000Z", "b", None),
        ]);
        let e = embeds(&[("a", 0.0), ("b", 30.0)]);
        let t = timeline(&set, &e, 500);
        assert_eq!(
            t.iter().map(|m| m.month.to_string()).collect::<Vec<_>>(),
            ["2025-11", "2025-12", "2026-01"],
            "관측이 없는 달도 빈 상태로 들어간다"
        );
        assert_eq!(t.iter().map(|m| m.n).collect::<Vec<_>>(), [1, 1, 2]);
    }

    #[test]
    fn missing_embedding_is_not_counted_toward_centroid() {
        // 임베딩이 없는 항목은 centroid의 표본으로 세지 않는다 — 3건 문턱은
        // 실제로 평균에 들어가는 벡터 수로 판정한다.
        let set = ObsSet::new(vec![
            ingest("2026-06-05T00:00:00.000Z", "a", None),
            ingest("2026-06-15T00:00:00.000Z", "b", None),
            ingest("2026-06-25T00:00:00.000Z", "c", None),
        ]);
        // "c"의 임베딩이 없다 → 6월의 |X| = 2 < 3.
        let e = embeds(&[("a", 0.0), ("b", 10.0)]);
        let s = state_at(&set, Month::new(2026, 6), &e, 500);
        assert_eq!(s.drift, None);
        assert_eq!(s.n, 3, "n은 임베딩 유무와 무관하다");
    }

    #[test]
    fn drift_is_cosine_distance_between_month_centroids() {
        // 6월 centroid는 0°, 7월 centroid는 90° 방향 → 코사인 거리 1.
        let set = ObsSet::new(vec![
            ingest("2026-06-01T00:00:00.000Z", "a1", None),
            ingest("2026-06-02T00:00:00.000Z", "a2", None),
            ingest("2026-06-03T00:00:00.000Z", "a3", None),
            ingest("2026-07-01T00:00:00.000Z", "b1", None),
            ingest("2026-07-02T00:00:00.000Z", "b2", None),
            ingest("2026-07-03T00:00:00.000Z", "b3", None),
        ]);
        let e = embeds(&[
            ("a1", -10.0),
            ("a2", 0.0),
            ("a3", 10.0),
            ("b1", 80.0),
            ("b2", 90.0),
            ("b3", 100.0),
        ]);
        let s = state_at(&set, Month::new(2026, 7), &e, 500);
        let d = s.drift.expect("양쪽 창 모두 3건이므로 drift가 있다");
        assert!((d - 1.0).abs() < 1e-5, "drift = {d}");
        assert_eq!(s.n, 6);

        // 같은 입력을 두 번 → 완전히 동일 (T22).
        assert_eq!(s, state_at(&set, Month::new(2026, 7), &e, 500));
    }
}
