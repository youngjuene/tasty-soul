//! `query_stats` 페이로드 (§11.2) 와 `soul stats` (§14).
//!
//! §11.2 — `query_stats`는 **인자를 받지 않는다.** §12의 각 지표는 자기 고유의 창을
//! 이미 갖고 있으므로 외부에서 창을 지정하면 정의가 충돌한다.
//!
//! §R11 — `soul stats`는 `prompt_sha256`이 바뀌는 관측 경계를 **반드시 출력한다.**

use super::Derived;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stats {
    #[serde(flatten)]
    pub derived: Derived,
    /// 종류별 관측 수.
    pub counts_by_type: std::collections::BTreeMap<String, usize>,
    /// `kind`별 활성 ingest 수.
    pub counts_by_kind: std::collections::BTreeMap<String, usize>,
    /// `quality`별 활성 ingest 수 (T57 검증용).
    pub counts_by_quality: std::collections::BTreeMap<String, usize>,
    /// 현재 군집 수. 없으면 `None`.
    pub cluster_k: Option<usize>,
}

pub fn build(derived: &Derived, set: &crate::obs::ObsSet, cluster_k: Option<usize>) -> Stats {
    use std::collections::BTreeMap;

    // 종류별 관측 수는 **전체 관측**을 센다. `soul stats`가 로그의 구성을 보여주는 값이므로
    // supersede된 ingest도 여기서는 보인다 (§R9는 §12의 파생 계산에만 적용된다).
    let mut counts_by_type: BTreeMap<String, usize> = BTreeMap::new();
    for o in set.as_slice() {
        *counts_by_type.entry(o.type_name().to_string()).or_insert(0) += 1;
    }

    // kind·quality는 **활성 ingest**만 센다 (§R9, T17·T57).
    let mut counts_by_kind: BTreeMap<String, usize> = BTreeMap::new();
    let mut counts_by_quality: BTreeMap<String, usize> = BTreeMap::new();
    for i in set.active_ingests() {
        *counts_by_kind
            .entry(i.source.kind.as_str().to_string())
            .or_insert(0) += 1;
        *counts_by_quality
            .entry(i.machine.quality.as_str().to_string())
            .or_insert(0) += 1;
    }

    Stats {
        derived: derived.clone(),
        counts_by_type,
        counts_by_kind,
        counts_by_quality,
        cluster_k,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ObsId;
    use crate::obs::{
        Axes, Ingest, Kind, Layer, Machine, ModelRef, ObsSet, Observation, Quality, Reading,
        Source, Verdict,
    };
    use crate::time::Ts;

    fn ts(s: &str) -> Ts {
        Ts::parse(s).expect("테스트 타임스탬프")
    }

    fn ingest(at: &str, kind: Kind, quality: Quality, supersedes: Option<ObsId>) -> Observation {
        Observation::Ingest(Ingest {
            id: crate::ids::new_id(),
            ts: ts(at),
            schema: crate::SCHEMA_VERSION,
            source: Source {
                kind,
                sha256: "0".into(),
                origin: "file:///x".into(),
                bytes: 1,
                mime: "image/jpeg".into(),
            },
            machine: Machine {
                prose: "서술".into(),
                axes: Axes::from_array([0.5; 8]),
                tags: vec![],
                quality,
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

    fn reading(at: &str, target: ObsId) -> Observation {
        Observation::Reading(Reading {
            id: crate::ids::new_id(),
            ts: ts(at),
            schema: crate::SCHEMA_VERSION,
            layer: Layer::Sensory,
            target,
            verdict: Verdict::Yes,
            prose: None,
            divergence: None,
        })
    }

    fn fixture() -> ObsSet {
        let old = ingest(
            "2026-06-01T00:00:00.000Z",
            Kind::Image,
            Quality::Minimal,
            None,
        );
        let old_id = old.id().clone();
        let redone = ingest(
            "2026-07-01T00:00:00.000Z",
            Kind::Image,
            Quality::Full,
            Some(old_id.clone()),
        );
        let text = ingest("2026-07-02T00:00:00.000Z", Kind::Text, Quality::Full, None);
        let r = reading("2026-07-03T00:00:00.000Z", old_id);
        ObsSet::new(vec![old, redone, text, r])
    }

    #[test]
    fn counts_by_type_covers_the_whole_log() {
        // 로그 구성이므로 supersede된 ingest도 보인다.
        let s = build(&Derived::default(), &fixture(), None);
        assert_eq!(s.counts_by_type.get("ingest"), Some(&3));
        assert_eq!(s.counts_by_type.get("reading"), Some(&1));
        assert_eq!(
            s.counts_by_type.get("context"),
            None,
            "없는 타입은 키도 없다"
        );
    }

    #[test]
    fn kind_and_quality_count_active_ingests_only() {
        // T17 — supersede된 minimal 항목이 quality 집계에 남으면 T57 검증이 틀어진다.
        let s = build(&Derived::default(), &fixture(), Some(4));
        assert_eq!(s.counts_by_kind.get("image"), Some(&1));
        assert_eq!(s.counts_by_kind.get("text"), Some(&1));
        assert_eq!(s.counts_by_quality.get("full"), Some(&2));
        assert_eq!(s.counts_by_quality.get("minimal"), None);
        assert_eq!(s.cluster_k, Some(4));
    }

    #[test]
    fn empty_set_gives_empty_maps() {
        let s = build(&Derived::default(), &ObsSet::default(), None);
        assert!(s.counts_by_type.is_empty());
        assert!(s.counts_by_kind.is_empty());
        assert!(s.counts_by_quality.is_empty());
        assert_eq!(s.cluster_k, None);
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn derived_is_flattened_into_the_payload() {
        // §11.2 — query_stats는 §12 지표 전부를 한 객체로 돌려준다.
        let mut d = Derived::default();
        d.observation_count = 7;
        let s = build(&d, &fixture(), None);
        let v = serde_json::to_value(&s).expect("직렬화");
        assert_eq!(v["observation_count"], 7);
        assert!(v.get("derived").is_none(), "중첩되지 않고 펼쳐져야 한다");
        assert!(v.get("counts_by_kind").is_some());
    }
}
