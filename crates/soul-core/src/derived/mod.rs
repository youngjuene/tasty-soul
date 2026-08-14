//! 파생 층 계산 (§12).
//!
//! **전부 결정론적이며 API를 호출하지 않는다** (임베딩 조회 제외).
//! 임베딩은 `EmbeddingLookup`으로 주입받는다 — 이 크레이트는 네트워크를 모른다.

pub mod axes;
pub mod cluster;
pub mod divergence;
pub mod pca;
pub mod state;
pub mod stats;
pub mod surprisal;

use crate::ids::ObsId;
use crate::obs::{Axes, Layer, ObsSet, PromptBoundary};
use crate::time::{Month, Ts};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// `machine.prose` 임베딩 조회기.
///
/// §12.7 — **군집·drift·crystal·surprisal은 `ingest.machine.prose` 임베딩만 쓴다.**
/// `context.critique`와 `layer=cultural` `reading.prose`의 임베딩은 별도 테이블에 있고
/// 이 조회기로는 나오지 않는다. 섞이면 군집이 "무엇을 좋아하는가"에서
/// "무엇에 관해 글이 쓰였는가"로 흐른다 (§18-3, T49).
pub trait EmbeddingLookup {
    /// 정규화 **이전**의 원본 벡터. 없으면 `None`.
    fn get(&self, text: &str) -> Option<Vec<f32>>;
}

impl EmbeddingLookup for BTreeMap<String, Vec<f32>> {
    fn get(&self, text: &str) -> Option<Vec<f32>> {
        BTreeMap::get(self, text).cloned()
    }
}

/// `SOUL.md`와 대시보드가 읽는 파생값 전부.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Derived {
    /// §R1 — 시간 기준점. 관측이 없으면 `None`.
    pub t_ref: Option<Ts>,
    pub t_first: Option<Ts>,
    /// supersede되지 않은 ingest 수 (§R9).
    pub observation_count: usize,
    /// 전체 관측 수(모든 타입). `SOUL.md` 헤더의 `관측`은 이 값이 아니라 ingest 수를 쓴다.
    pub total_observation_count: usize,

    /// §12.1 — `clamp(computed + offset, 0, 1)`. **표시하는 값은 이것이다.**
    pub axes_final: Option<Axes>,
    /// §12.1 — 관측 가중 평균만.
    pub axes_computed: Option<Axes>,
    /// §12.1 — 모든 `soul_delta`의 `axis_delta` 합.
    pub axes_offset: Axes,
    /// §12.2 — 90일 변화. 어느 창이든 3건 미만이면 축별로 `None`.
    /// **`offset`을 더하지 않는다** — 의도된 비대칭이다.
    pub axes_change: [Option<f64>; 8],

    /// §12.6 — 2×2 셀 개수.
    pub cells: CellCounts,
    /// §12.6 — 최근 30일 셀 추이 (대시보드 패널 2의 화살표용).
    pub cells_recent30: CellCounts,
    /// §8.2 `정정 N건` — `prose != null` 인 reading 수(두 층 합산).
    ///
    /// `Coherence.sample` 로 대신 세면 `|R| < 2` 일 때 `Coherence` 자체가 `None` 이라
    /// 정정이 실제로 1건 있어도 `0건` 으로 렌더된다. 개수는 측정된 값이므로
    /// 별도 필드로 센다 (§R10의 null 과 구분된다).
    pub corrections_total: usize,
    /// §12.6 — layer별 30일 divergence 평균.
    pub divergence_sensory: Option<f64>,
    pub divergence_cultural: Option<f64>,
    /// §12.6 — 체계적 어긋남. layer별로 따로 계산한다.
    pub coherence_sensory: Option<Coherence>,
    pub coherence_cultural: Option<Coherence>,

    /// §12.5 — 월별 상태. `M` 전체.
    pub timeline: Vec<MonthState>,
    /// §8.2 헤더의 `해상도` = `T_ref`가 속한 달의 `crystal`.
    pub crystal_now: Option<f64>,
    /// §8.2 헤더의 `어긋남` = `read`가 아닌 셀의 비율.
    pub misread_ratio: Option<f64>,

    /// §R11 — `prompt_sha256` 경계. 대시보드 패널 1·3이 세로 구분선을 긋는다.
    /// `ingest`(`describe.md`)와 `soul_delta`(`reflect.md`)를 **둘 다** 싣는다 — 어느
    /// 프롬프트가 바뀐 경계인지 말할 수 없으면 구분선이 아무 뜻도 갖지 못한다.
    pub prompt_boundaries: Vec<PromptBoundary>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellCounts {
    pub read: usize,
    pub other_reason: usize,
    pub wrong_words: usize,
    pub unread: usize,
}

impl CellCounts {
    pub fn total(&self) -> usize {
        self.read + self.other_reason + self.wrong_words + self.unread
    }
    pub fn get(&self, c: Cell) -> usize {
        match c {
            Cell::Read => self.read,
            Cell::OtherReason => self.other_reason,
            Cell::WrongWords => self.wrong_words,
            Cell::Unread => self.unread,
        }
    }
    pub fn bump(&mut self, c: Cell) {
        match c {
            Cell::Read => self.read += 1,
            Cell::OtherReason => self.other_reason += 1,
            Cell::WrongWords => self.wrong_words += 1,
            Cell::Unread => self.unread += 1,
        }
    }
}

/// §12.6의 2×2 셀.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cell {
    /// (yes, yes) — 기계가 이 사람을 읽었다.
    Read,
    /// (yes, no) — **보는 것은 같으나 끌리는 이유가 다르다.** 이 시스템이 찾으려는 것.
    OtherReason,
    /// (no, yes) — 서술은 빗나갔어도 무엇이 중요한지는 통한다.
    WrongWords,
    /// (no, no) — 아직 못 잡았다. 성찰 에이전트가 최우선으로 볼 항목.
    Unread,
}

impl Cell {
    pub const ALL: [Cell; 4] = [
        Cell::Read,
        Cell::OtherReason,
        Cell::WrongWords,
        Cell::Unread,
    ];
    pub fn as_str(&self) -> &'static str {
        match self {
            Cell::Read => "read",
            Cell::OtherReason => "other_reason",
            Cell::WrongWords => "wrong_words",
            Cell::Unread => "unread",
        }
    }
    pub fn parse(s: &str) -> Option<Cell> {
        Cell::ALL.into_iter().find(|c| c.as_str() == s)
    }
}

/// §12.6 — 방향의 일관성. 창은 **전체 기간**이다.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Coherence {
    /// `‖mean_v‖ / mean(‖v_r‖)`. 0 = 무작위, 1 = 완전 일관. `|R| < 2`면 이 구조체 자체가 `None`.
    pub value: f64,
    /// `|R|` — `prose`가 있는 reading 수.
    pub sample: usize,
    /// `v_r`이 `mean_v`와 코사인 유사도가 가장 높은 reading 3개의 **`target` ID**.
    pub examples: Vec<ObsId>,
    /// `value >= 0.4 && sample >= 5` 이면 체계적 어긋남이 존재한다고 본다.
    pub systematic: bool,
}

/// §12.5 `state_at(month)`의 반환값.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MonthState {
    pub month: Month,
    /// 이 달과 직전 달 centroid 사이의 코사인 거리. 어느 쪽이든 3건 미만이면 `None`.
    pub drift: Option<f64>,
    /// 누적 집합 군집의 실루엣 계수 평균. 군집이 없으면 `None`.
    pub crystal: Option<f64>,
    /// 이 달 **말까지의 누적** ingest 수.
    pub n: usize,
}

/// §R9 순서대로의 활성 ingest `machine.prose` 임베딩. 하나라도 캐시에 없으면 `None`.
///
/// 군집 입력은 **활성 ingest 의 서술문뿐**이다 (§12.7). `obs_vec` ID 색인이 아니라
/// 텍스트 키 캐시로 조회하므로, `--from-scratch` 가 색인을 비운 뒤에도 동작한다.
pub fn object_vectors(
    db: &crate::db::Db,
    set: &ObsSet,
    cfg: &crate::config::Config,
) -> Option<Vec<Vec<f32>>> {
    let mut out = Vec::new();
    for i in set.active_ingests() {
        let key = crate::db::embed_cache::cache_key(
            crate::rebuild::EMBED_PROVIDER,
            &cfg.embed.model,
            cfg.embed.dims,
            &i.machine.prose,
        );
        match db.embed_get(&key) {
            Ok(Some(v)) => out.push(v),
            _ => return None,
        }
    }
    Some(out)
}

/// 현재 군집 수 (§12.3). **모든 표면이 이 함수를 쓴다.**
///
/// 캐시(`cluster_cache`)만 읽으면 `soul rebuild --from-scratch` 직후처럼 캐시가 빈
/// 상태에서 `null` 이 나온다. 그런데 같은 저장소를 두고 CLI 는 8, MCP 는 null 을
/// 말하는 상황이 실제로 있었다 — **같은 사실에 두 답이 나오면 둘 다 못 믿는다.**
///
/// 그래서 순서를 하나로 못박는다: 임베딩이 다 있으면 계산하고, 아니면 캐시로 물러난다.
/// k-means 는 O(n) 이라 5,000건에서도 읽기 경로 예산 안이다 (§20.1).
pub fn cluster_k(
    db: &crate::db::Db,
    set: &ObsSet,
    cfg: &crate::config::Config,
) -> crate::Result<Option<usize>> {
    if let Some(vectors) = object_vectors(db, set, cfg) {
        if let Some(c) = cluster::cluster(&vectors) {
            return Ok(Some(c.k));
        }
        // `n < 4` 면 군집이 없는 것이 정답이다 (§12.3). 캐시로 물러나지 않는다.
        if vectors.len() < 4 {
            return Ok(None);
        }
    }
    Ok(db.cluster_get()?.map(|(_, c)| c.k))
}

/// 파생 층 전체를 한 번에 계산한다 (`soul rebuild`·`soul render`의 유일한 진입점).
///
/// 증분 경로(§20.7)는 이것을 부르지 않고 누산기를 갱신하지만, 결과는 반드시 일치해야 한다.
pub fn compute(
    set: &ObsSet,
    embeds: &dyn EmbeddingLookup,
    silhouette_max_samples: usize,
) -> Derived {
    // §R9 — §12의 모든 계산은 active_ingests()를 거친다 (T17).
    let active = set.active_ingests();

    let mut d = Derived {
        // §R1 — 시간 기준점은 wall clock이 아니라 T_ref다. 여기서 now()를 부르지 않는다 (T14).
        t_ref: set.t_ref(),
        t_first: set.t_first(),
        observation_count: active.len(),
        total_observation_count: set.len(),
        // §R11 — 프롬프트 경계는 관측이 없어도 빈 목록으로 성립한다.
        prompt_boundaries: set.prompt_boundaries(),
        ..Derived::default()
    };

    // 관측이 하나도 없으면 나머지는 전부 null/0이다. 여기서 끝낸다.
    let Some(t_ref) = d.t_ref else {
        return d;
    };

    // §12.1 — 축 값. 이것이 유일한 계산 지점이다.
    let computed = axes::weighted_mean(&active, t_ref, true);
    let offset = axes::offset(set);
    d.axes_computed = Some(computed);
    d.axes_offset = offset;
    d.axes_final = Some(axes::finalize(computed, offset));
    // §12.2 — offset을 더하지 않는다. 의도된 비대칭이다.
    d.axes_change = axes::change_90d(set, t_ref);

    // §12.6 — 셀이 주 지표, 실수 지표는 보조.
    d.cells = divergence::cell_counts(set, None);
    d.cells_recent30 = divergence::cell_counts(set, Some(t_ref.minus_days(30)));
    d.divergence_sensory = divergence::divergence_mean(set, Layer::Sensory, t_ref);
    d.divergence_cultural = divergence::divergence_mean(set, Layer::Cultural, t_ref);
    d.coherence_sensory = divergence::coherence(set, Layer::Sensory, embeds);
    d.coherence_cultural = divergence::coherence(set, Layer::Cultural, embeds);
    d.misread_ratio = divergence::misread_ratio(&d.cells);
    // §8.2 — 정정은 개수다. 기간 제한 없이 두 층을 합쳐 센다 (§12.6의 `R` 과 같은 모집단).
    d.corrections_total = set.readings().iter().filter(|r| r.prose.is_some()).count();

    // §12.5 — 월별 상태. `해상도`는 T_ref가 속한 달의 crystal이다 (§8.2).
    d.timeline = state::timeline(set, embeds, silhouette_max_samples);
    let now_month = t_ref.month();
    d.crystal_now = d
        .timeline
        .iter()
        .find(|m| m.month == now_month)
        .and_then(|m| m.crystal);

    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obs::{
        BlockDelta, Ingest, Kind, Machine, ModelRef, Observation, Quality, Source, Window,
    };

    fn ts(s: &str) -> Ts {
        Ts::parse(s).expect("테스트 타임스탬프")
    }

    fn model() -> ModelRef {
        ModelRef {
            provider: "test".into(),
            id: "m".into(),
            prompt_sha256: None,
            calls: vec![],
        }
    }

    fn ingest(at: &str, prose: &str, sha: &str, supersedes: Option<ObsId>) -> Observation {
        Observation::Ingest(Ingest {
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
                axes: Axes::from_array([0.4; 8]),
                tags: vec![],
                quality: Quality::Full,
                prompt_sha256: sha.into(),
            },
            min_dist: None,
            surprisal: 1.0,
            model: model(),
            supersedes,
        })
    }

    fn soul_delta(at: &str, from: ObsId, to: ObsId, delta: f64) -> Observation {
        let mut axis_delta = BTreeMap::new();
        axis_delta.insert("chroma".to_string(), delta);
        let mut blocks = BTreeMap::new();
        blocks.insert(
            "soul:profile".to_string(),
            BlockDelta {
                from_hash: "h".into(),
                to_text: "t".into(),
            },
        );
        Observation::SoulDelta(crate::obs::SoulDelta {
            id: crate::ids::new_id(),
            ts: ts(at),
            schema: crate::SCHEMA_VERSION,
            window: Window { from, to },
            blocks,
            axis_delta,
            morphology_delta: None,
            cites: vec![],
            rationale: "테스트".into(),
            model: model(),
        })
    }

    /// 3건 — §12.3의 군집 문턱(4건) 아래라 crystal은 항상 null이다.
    fn fixture() -> ObsSet {
        let a = ingest("2026-06-10T00:00:00.000Z", "a", "p1", None);
        let b = ingest("2026-07-01T00:00:00.000Z", "b", "p1", None);
        let c = ingest("2026-07-05T00:00:00.000Z", "c", "p2", None);
        let d = soul_delta(
            "2026-07-06T00:00:00.000Z",
            a.id().clone(),
            c.id().clone(),
            0.25,
        );
        ObsSet::new(vec![a, b, c, d])
    }

    fn embeds() -> BTreeMap<String, Vec<f32>> {
        [("a", 0.0f32), ("b", 1.0), ("c", 2.0)]
            .into_iter()
            .map(|(k, x)| (k.to_string(), vec![1.0, x]))
            .collect()
    }

    #[test]
    fn empty_set_yields_all_none_and_does_not_crash() {
        let d = compute(&ObsSet::default(), &BTreeMap::new(), 500);
        assert_eq!(d, Derived::default());
        assert_eq!(d.t_ref, None);
        assert_eq!(d.axes_final, None);
        assert_eq!(d.observation_count, 0);
        assert_eq!(d.cells.total(), 0);
        assert_eq!(d.misread_ratio, None);
        assert!(d.timeline.is_empty());
        assert!(d.prompt_boundaries.is_empty());
    }

    #[test]
    fn derived_is_independent_of_wall_clock() {
        // T14 — 같은 ObsSet이면 언제 실행하든 같은 값이다. now()를 쓰면 깨진다.
        let set = fixture();
        let e = embeds();
        let a = compute(&set, &e, 500);
        let b = compute(&set, &e, 500);
        assert_eq!(a, b);
        assert_eq!(
            a.t_ref,
            Some(ts("2026-07-06T00:00:00.000Z")),
            "§R1 — 기준점은 wall clock이 아니라 가장 늦은 ts다"
        );
        assert_eq!(a.t_first, Some(ts("2026-06-10T00:00:00.000Z")));
    }

    #[test]
    fn counts_split_active_ingests_from_all_observations() {
        // T17 — supersede된 ingest는 observation_count에서 빠지지만
        // total_observation_count(로그 전체)에는 남는다.
        let old = ingest("2026-06-01T00:00:00.000Z", "old", "p1", None);
        let old_id = old.id().clone();
        let set = ObsSet::new(vec![
            old,
            ingest("2026-07-01T00:00:00.000Z", "new", "p1", Some(old_id)),
        ]);
        let d = compute(&set, &embeds(), 500);
        assert_eq!(d.observation_count, 1);
        assert_eq!(d.total_observation_count, 2);
    }

    #[test]
    fn axes_final_is_computed_plus_offset_clamped() {
        // §12.1 — final = clamp(computed + offset, 0, 1). 여기서만 합성한다 (T15).
        let d = compute(&fixture(), &embeds(), 500);
        let computed = d.axes_computed.expect("관측이 있으면 computed가 있다");
        let final_ = d.axes_final.expect("관측이 있으면 final이 있다");
        assert_eq!(d.axes_offset.chroma, 0.25);
        assert!((final_.chroma - (computed.chroma + 0.25).clamp(0.0, 1.0)).abs() < 1e-12);
        assert!(final_.to_array().iter().all(|v| (0.0..=1.0).contains(v)));
    }

    #[test]
    fn crystal_now_is_the_t_ref_month() {
        // §8.2 — 헤더의 `해상도`는 T_ref가 속한 달의 crystal이다.
        let set = fixture();
        let d = compute(&set, &embeds(), 500);
        let last = d.timeline.last().expect("타임라인이 비지 않는다");
        assert_eq!(last.month, Month::new(2026, 7));
        assert_eq!(d.crystal_now, last.crystal);
        assert_eq!(
            d.timeline.iter().map(|m| m.n).collect::<Vec<_>>(),
            [1, 3],
            "누적이므로 단조 증가한다"
        );
    }

    #[test]
    fn prompt_boundaries_are_carried_through() {
        // §R11 — 프롬프트가 바뀌는 지점을 파생값에 실어 대시보드가 구분선을 긋는다.
        let d = compute(&fixture(), &embeds(), 500);
        assert_eq!(
            d.prompt_boundaries
                .iter()
                .map(|b| b.sha256.as_str())
                .collect::<Vec<_>>(),
            ["p1", "p2"]
        );
    }

    #[test]
    fn cells_are_empty_without_readings() {
        // 응답이 없으면 셀은 하나도 정해지지 않고, 어긋남 비율은 null이다 (§R10).
        let d = compute(&fixture(), &embeds(), 500);
        assert_eq!(d.cells.total(), 0);
        assert_eq!(d.cells_recent30.total(), 0);
        assert_eq!(d.misread_ratio, None);
        assert_eq!(d.divergence_sensory, None);
        assert_eq!(d.divergence_cultural, None);
    }

    // ──────────────────────────────────── object_vectors · cluster_k (§R9 · §12.3)

    /// 임베딩 캐시를 워밍한 메모리 DB.
    fn warm_db(texts: &[&str], cfg: &crate::config::Config) -> crate::db::Db {
        let db = crate::db::Db::open_in_memory().expect("db");
        for t in texts {
            let key = crate::db::embed_cache::cache_key(
                crate::rebuild::EMBED_PROVIDER,
                &cfg.embed.model,
                cfg.embed.dims,
                t,
            );
            // 내용은 중요하지 않다. 차원만 맞으면 된다.
            let v: Vec<f32> = (0..cfg.embed.dims)
                .map(|i| ((i as f32) * 0.01 + t.len() as f32).sin())
                .collect();
            db.embed_put(&key, cfg.embed.dims, &v).expect("put");
        }
        db
    }

    /// §R9 — supersede된 ingest 는 `active_ingests()` 가 걸러 낸다. 그 항목의 임베딩이
    /// 캐시에 없어도 `None` 이 되면 안 된다. 되면 recast 한 번에 군집이 영구히 `—` 가 된다.
    #[test]
    fn object_vectors_ignores_superseded_ingests() {
        let cfg = crate::config::Config::default();
        let old = ingest("2026-03-01T00:00:00.000Z", "낡은 서술", "a", None);
        let new = ingest(
            "2026-03-02T00:00:00.000Z",
            "다시 쓴 서술",
            "a",
            Some(old.id().clone()),
        );
        let set = ObsSet::new(vec![old, new]);
        // **새 것만** 워밍한다.
        let db = warm_db(&["다시 쓴 서술"], &cfg);
        let v = object_vectors(&db, &set, &cfg).expect("활성 ingest 것만 있으면 충분하다");
        assert_eq!(v.len(), 1);
    }

    /// 활성 ingest 중 하나라도 미스면 `None` 이다 — 부분 집합으로 군집을 만들지 않는다 (§R5).
    #[test]
    fn object_vectors_is_none_when_any_active_ingest_misses() {
        let cfg = crate::config::Config::default();
        let set = ObsSet::new(vec![
            ingest("2026-03-01T00:00:00.000Z", "첫째", "a", None),
            ingest("2026-03-02T00:00:00.000Z", "둘째", "a", None),
        ]);
        let db = warm_db(&["첫째"], &cfg);
        assert!(object_vectors(&db, &set, &cfg).is_none());
    }

    /// 캐시가 비어 있어도 계산해서 답한다.
    ///
    /// 캐시만 읽던 시절에는 `soul rebuild --from-scratch` 직후 같은 저장소를 두고
    /// CLI 는 `군집 8`, MCP 는 `null` 을 말했다. **같은 사실에 두 답이 나오면 둘 다 못 믿는다.**
    #[test]
    fn cluster_k_is_computed_when_the_cache_is_empty() {
        let cfg = crate::config::Config::default();
        let texts: Vec<String> = (0..12).map(|i| format!("서술 {i}")).collect();
        let obs: Vec<Observation> = texts
            .iter()
            .enumerate()
            .map(|(i, t)| ingest(&format!("2026-03-{:02}T00:00:00.000Z", i + 1), t, "a", None))
            .collect();
        let set = ObsSet::new(obs);
        let refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
        let db = warm_db(&refs, &cfg);

        assert!(db.cluster_get().unwrap().is_none(), "캐시는 비어 있다");
        let k = cluster_k(&db, &set, &cfg).unwrap();
        // n=12 → k = clamp(round(sqrt(6)), 2, 8) = 2
        assert_eq!(k, Some(cluster::choose_k(12)));
    }

    /// `n < 4` 면 군집이 없는 것이 정답이다 (§12.3). 캐시로 물러나지 않는다.
    #[test]
    fn cluster_k_is_none_below_four_observations() {
        let cfg = crate::config::Config::default();
        let set = ObsSet::new(vec![
            ingest("2026-03-01T00:00:00.000Z", "하나", "a", None),
            ingest("2026-03-02T00:00:00.000Z", "둘", "a", None),
        ]);
        let db = warm_db(&["하나", "둘"], &cfg);
        assert_eq!(cluster_k(&db, &set, &cfg).unwrap(), None);
    }
}
