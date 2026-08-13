//! 관측 저장소 — 월별 샤딩 읽기/쓰기와 재생 순서 (§3 · §20.6 · §R2).
//!
//! ## 구현 지시
//!
//! - 파일명은 `<ULID>.json`, 디렉토리는 `observations/YYYY-MM/`(관측의 `ts` 기준 UTC 월).
//! - 쓰기는 `Observation::to_canonical_json` 결과를 그대로 쓴다 (§R6). 다른 직렬화 경로를 만들지 않는다.
//! - 재생 순서는 **ULID 사전순**이다. 디렉토리 경로는 순서에 영향을 주지 않는다.
//! - 관측은 불변이다. 이미 존재하는 ID로 `append`하면 에러다.
//! - `ObsSet::active_ingests`가 §R9(supersedes 제외)의 **유일한 관문**이다.
//!   §12의 계산은 전부 이 함수를 거쳐야 한다. 다른 곳에서 `ingests()`를 직접 쓰면
//!   재분석 항목이 이중 계상되어 축과 군집이 조용히 기운다 (§18-2, T17).

use crate::error::{Result, SoulError};
use crate::ids::ObsId;
use crate::obs::model::*;
use crate::paths::Paths;
use crate::time::Ts;
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

/// 디스크 위의 관측 로그.
pub struct Store {
    paths: Paths,
    /// ULID → 파일 경로. 최초 조회 시 한 번 스캔해 채운다.
    index: std::sync::RwLock<Option<BTreeMap<ObsId, PathBuf>>>,
}

impl Store {
    pub fn new(paths: Paths) -> Store {
        Store {
            paths,
            index: std::sync::RwLock::new(None),
        }
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    /// 인덱스를 강제로 다시 만든다. 외부에서 파일이 추가된 뒤 호출한다.
    pub fn invalidate(&self) {
        *self.index.write().unwrap() = None;
    }

    /// 관측을 기록한다. 반환값은 쓰인 파일 경로.
    ///
    /// **커밋은 하지 않는다.** 커밋은 호출자(§R8의 쓰기 단위)가 `git` 모듈로 한다.
    pub fn append(&self, obs: &Observation) -> Result<PathBuf> {
        // 기록 직전에 구조 불변식을 통과해야 한다 (§6).
        obs.validate()?;

        let id = obs.id();
        // 디렉토리는 관측의 `ts` 기준 UTC 월이다 (§20.6). ULID의 시각이 아니다.
        let path = self.paths.observation_file(obs.ts(), id);

        // 관측은 불변이다 (§R2·§R9). 인덱스와 실제 파일을 모두 본다 —
        // 인덱스가 낡았더라도 이미 있는 파일을 덮어쓰지 않게 한다.
        if self.exists(id) || path.exists() {
            return Err(SoulError::invalid(format!(
                "관측 {id} 이(가) 이미 있습니다. 관측은 불변입니다"
            )));
        }

        // §R6 — 다른 직렬화 경로를 만들지 않는다.
        let json = obs.to_canonical_json()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, json.as_bytes())?;

        // 인덱스가 이미 만들어져 있으면 전체 재스캔 없이 갱신한다.
        let mut guard = self.index.write().unwrap_or_else(|e| e.into_inner());
        if let Some(ix) = guard.as_mut() {
            ix.insert(id.clone(), path.clone());
        }

        Ok(path)
    }

    pub fn read(&self, id: &ObsId) -> Result<Observation> {
        let path = self
            .with_index(|ix| ix.get(id).cloned())?
            .ok_or_else(|| SoulError::ObsNotFound(id.to_string()))?;
        let text = std::fs::read_to_string(&path)?;
        Observation::from_json(&text)
    }

    pub fn exists(&self, id: &ObsId) -> bool {
        // 스캔에 실패하면 "모른다"가 아니라 "없다"로 답한다. 판단은 호출자가
        // `read`/`append`에서 에러로 다시 만난다.
        self.with_index(|ix| ix.contains_key(id)).unwrap_or(false)
    }

    /// ULID 오름차순 전체 관측.
    pub fn all(&self) -> Result<Vec<Observation>> {
        // 인덱스는 `BTreeMap<ObsId, _>`이므로 순회 순서가 곧 ULID 사전순이다.
        let paths = self.with_index(|ix| ix.values().cloned().collect::<Vec<_>>())?;
        let mut out = Vec::with_capacity(paths.len());
        for p in paths {
            let text = std::fs::read_to_string(&p)?;
            out.push(Observation::from_json(&text)?);
        }
        Ok(out)
    }

    /// ULID 오름차순 ID 목록. 전체 역직렬화 없이 셀 때 쓴다.
    pub fn ids(&self) -> Result<Vec<ObsId>> {
        self.with_index(|ix| ix.keys().cloned().collect())
    }

    pub fn count(&self) -> Result<usize> {
        self.ids().map(|v| v.len())
    }

    /// 전부 읽어 `ObsSet`으로 만든다.
    pub fn load_set(&self) -> Result<ObsSet> {
        Ok(ObsSet::new(self.all()?))
    }

    /// 인덱스를 (필요하면 한 번 만들어서) 빌려준다.
    fn with_index<T>(&self, f: impl FnOnce(&BTreeMap<ObsId, PathBuf>) -> T) -> Result<T> {
        {
            let guard = self.index.read().unwrap_or_else(|e| e.into_inner());
            if let Some(ix) = guard.as_ref() {
                return Ok(f(ix));
            }
        }
        let mut guard = self.index.write().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some(self.scan()?);
        }
        let ix = guard
            .as_ref()
            .ok_or_else(|| SoulError::invalid("관측 인덱스를 만들지 못했습니다"))?;
        Ok(f(ix))
    }

    /// `observations/*/*.json` 을 한 번 훑어 ULID → 경로 표를 만든다.
    ///
    /// 디렉토리가 아직 없으면 빈 인덱스다 (최초 실행). 파일명이 ULID가 아니면
    /// 관측이 아니므로 건너뛴다 — 편집기 임시 파일 하나가 저장소 전체를
    /// 못 읽게 만들면 안 된다.
    fn scan(&self) -> Result<BTreeMap<ObsId, PathBuf>> {
        let mut out = BTreeMap::new();
        let root = self.paths.observations();
        let months = match std::fs::read_dir(&root) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e.into()),
        };
        for month in months {
            let month = month?;
            if !month.file_type()?.is_dir() {
                continue;
            }
            for entry in std::fs::read_dir(month.path())? {
                let path = entry?.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                if let Ok(id) = ObsId::parse(stem) {
                    out.insert(id, path);
                }
            }
        }
        Ok(out)
    }
}

/// 메모리에 올린 관측 집합. §12의 모든 계산이 이것을 입력으로 받는다.
#[derive(Clone, Debug, Default)]
pub struct ObsSet {
    all: Vec<Observation>,
}

impl ObsSet {
    /// ULID 오름차순으로 정렬해서 보관한다.
    pub fn new(mut all: Vec<Observation>) -> ObsSet {
        all.sort_by(|a, b| a.id().cmp(b.id()));
        ObsSet { all }
    }

    pub fn as_slice(&self) -> &[Observation] {
        &self.all
    }

    pub fn len(&self) -> usize {
        self.all.len()
    }

    pub fn is_empty(&self) -> bool {
        self.all.is_empty()
    }

    pub fn get(&self, id: &ObsId) -> Option<&Observation> {
        self.all
            .binary_search_by(|o| o.id().cmp(id))
            .ok()
            .map(|i| &self.all[i])
    }

    /// §R1 — 전체 관측 중 가장 늦은 `ts`. **모든 파생값의 시간 기준점이다.**
    ///
    /// 주의: ULID 최대값이 아니라 `ts` 최대값이다. 둘은 거의 항상 같지만
    /// 시스템 시계가 뒤로 조정되면 갈라진다.
    pub fn t_ref(&self) -> Option<Ts> {
        self.all.iter().map(|o| o.ts()).max()
    }

    /// 최초 관측의 `ts` (§8.2 헤더의 `최초`).
    pub fn t_first(&self) -> Option<Ts> {
        self.all.iter().map(|o| o.ts()).min()
    }

    /// **supersede된 것을 포함한** 모든 ingest. 아카이브 원본 조회에만 쓴다.
    pub fn ingests(&self) -> Vec<&Ingest> {
        self.all.iter().filter_map(|o| o.as_ingest()).collect()
    }

    /// 다른 관측이 `supersedes`로 가리키는 ingest ID 집합 (§R9).
    pub fn superseded_ids(&self) -> HashSet<ObsId> {
        self.all
            .iter()
            .filter_map(|o| o.as_ingest())
            .filter_map(|i| i.supersedes.clone())
            .collect()
    }

    /// **§12의 `I`.** supersede되지 않은 ingest만, ULID 오름차순.
    ///
    /// §12의 모든 계산과 §13 화면 6의 표시가 이것을 쓴다 (T17·T70b).
    pub fn active_ingests(&self) -> Vec<&Ingest> {
        let dead = self.superseded_ids();
        self.all
            .iter()
            .filter_map(|o| o.as_ingest())
            .filter(|i| !dead.contains(&i.id))
            .collect()
    }

    pub fn readings(&self) -> Vec<&Reading> {
        self.all.iter().filter_map(|o| o.as_reading()).collect()
    }

    pub fn contexts(&self) -> Vec<&ContextObs> {
        self.all.iter().filter_map(|o| o.as_context()).collect()
    }

    pub fn soul_deltas(&self) -> Vec<&SoulDelta> {
        self.all.iter().filter_map(|o| o.as_soul_delta()).collect()
    }

    pub fn profile_edits(&self) -> Vec<&ProfileEdit> {
        self.all
            .iter()
            .filter_map(|o| o.as_profile_edit())
            .collect()
    }

    /// 이 ingest를 target으로 하는 `context` 중 ULID 최대 (§12.6 — 재요청 시 최신만).
    pub fn latest_context_for(&self, ingest: &ObsId) -> Option<&ContextObs> {
        self.contexts()
            .into_iter()
            .filter(|c| &c.target == ingest)
            .max_by(|a, b| a.id.cmp(&b.id))
    }

    /// 이 target을 가리키는 해당 layer의 `reading` 중 ULID 최대.
    pub fn latest_reading_for(&self, target: &ObsId, layer: Layer) -> Option<&Reading> {
        self.readings()
            .into_iter()
            .filter(|r| &r.target == target && r.layer == layer)
            .max_by(|a, b| a.id.cmp(&b.id))
    }

    /// `id`보다 ULID가 큰 첫 관측 (§11.2의 `window.from` 계산에 쓴다).
    pub fn first_after(&self, id: &ObsId) -> Option<&Observation> {
        self.all.iter().find(|o| o.id() > id)
    }

    /// 마지막 `soul_delta` (§11.2 트리거 판정).
    pub fn last_soul_delta(&self) -> Option<&SoulDelta> {
        self.soul_deltas()
            .into_iter()
            .max_by(|a, b| a.id.cmp(&b.id))
    }

    /// 마지막 `soul_delta` 이후 누적된 `ingest` 수 (§11.2 트리거).
    pub fn ingests_since_last_delta(&self) -> usize {
        let since = self.last_soul_delta().map(|d| d.id.clone());
        self.all
            .iter()
            .filter(|o| matches!(o, Observation::Ingest(_)))
            .filter(|o| since.as_ref().map(|s| o.id() > s).unwrap_or(true))
            .count()
    }

    /// §R11 — `prompt_sha256`이 바뀌는 관측 경계. `soul stats`가 반드시 출력한다.
    /// 반환값은 "이 ID부터 새 프롬프트"인 목록이며, 종류마다 첫 관측도 포함한다.
    pub fn prompt_boundaries(&self) -> Vec<PromptBoundary> {
        let mut out = Vec::new();

        // 두 종류를 **한 줄로 섞어 훑지 않는다.** `ingest`는 `describe.md`의 해시를,
        // `soul_delta`는 `reflect.md`의 해시를 들고 있어 서로 비교할 수 있는 값이 아니다.
        // 섞으면 두 종류가 번갈아 나올 때마다 값이 달라 보여 가짜 경계가 쏟아진다.
        // 종류별로 따로 훑고 마지막에 ULID 순으로 합친다.

        // §R9 — supersede된 ingest는 §12의 다른 계산과 마찬가지로 여기서도 빠진다.
        let mut cur: Option<&str> = None;
        for i in self.active_ingests() {
            let s = i.machine.prompt_sha256.as_str();
            if cur != Some(s) {
                out.push(PromptBoundary {
                    id: i.id.clone(),
                    kind: "ingest".to_string(),
                    sha256: s.to_string(),
                });
                cur = Some(s);
            }
        }

        // §R11은 `soul_delta.model.prompt_sha256`도 **이름으로** 적고 있다. 성찰 프롬프트를
        // 고치면 제안의 어휘와 축 변화폭이 통째로 움직이는데, 여기를 보지 않으면 그 이동이
        // 아무 표시 없이 취향의 변화로 남는다.
        let mut cur: Option<&str> = None;
        for d in self.soul_deltas() {
            // 해시가 없는 델타는 "프롬프트가 바뀌었다"고 말할 근거가 아니다. 없는 것을
            // 하나의 값으로 세면 있지도 않은 경계가 생긴다 — 누락은 `soul doctor`의 몫이다.
            let Some(s) = d.model.prompt_sha256.as_deref() else {
                continue;
            };
            if cur != Some(s) {
                out.push(PromptBoundary {
                    id: d.id.clone(),
                    kind: "soul_delta".to_string(),
                    sha256: s.to_string(),
                });
                cur = Some(s);
            }
        }

        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }
}

/// §R11 — "이 관측부터 프롬프트가 달라졌다"는 지점 하나.
///
/// **왜 `kind`가 붙어 있는가.** §R11은 `machine.prompt_sha256`(투입 경로, `describe.md`)과
/// `soul_delta.model.prompt_sha256`(성찰 경로, `reflect.md`)을 **둘 다** 이름으로 적고 있다.
/// 두 값은 서로 다른 파일의 해시라 비교 가능한 값이 아니다. 종류를 잃어버리면 `soul stats`도
/// 대시보드도 어느 프롬프트가 바뀐 경계인지 말할 수 없고, 한 줄로 섞어 훑는 순간
/// 두 종류가 번갈아 나올 때마다 가짜 경계가 쏟아진다 — §R11이 막으려는 바로 그 오류다.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PromptBoundary {
    pub id: ObsId,
    /// `"ingest"` 또는 `"soul_delta"` — `Observation::type_name()`과 같은 이름을 쓴다.
    pub kind: String,
    pub sha256: String,
}

impl From<Vec<Observation>> for ObsSet {
    fn from(v: Vec<Observation>) -> Self {
        ObsSet::new(v)
    }
}

/// 존재하지 않는 ID를 참조하는 관측을 찾는다. `soul doctor`가 쓴다.
pub fn dangling_targets(set: &ObsSet) -> Vec<(ObsId, ObsId)> {
    let mut out = Vec::new();
    for o in set.as_slice() {
        let target = match o {
            Observation::Reading(r) => Some(&r.target),
            Observation::Context(c) => Some(&c.target),
            Observation::Reaction(r) => Some(&r.target),
            _ => None,
        };
        if let Some(t) = target {
            if set.get(t).is_none() {
                out.push((o.id().clone(), t.clone()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;

    const ID_A: &str = "01AAAAAAAAAAAAAAAAAAAAAAAA";
    const ID_B: &str = "01BBBBBBBBBBBBBBBBBBBBBBBB";
    const ID_C: &str = "01CCCCCCCCCCCCCCCCCCCCCCCC";
    const ID_D: &str = "01DDDDDDDDDDDDDDDDDDDDDDDD";
    const ID_E: &str = "01EEEEEEEEEEEEEEEEEEEEEEEE";

    fn temp_store(tag: &str) -> (Store, Paths) {
        let root = std::env::temp_dir().join(format!("tasty-soul-{tag}-{}", crate::ids::new_id()));
        let paths = Paths::at(root);
        paths.ensure_dirs().expect("디렉토리 생성");
        (Store::new(paths.clone()), paths)
    }

    fn reaction(id: &str, ts: &str) -> Observation {
        Observation::Reaction(Reaction {
            id: ObsId::parse(id).expect("ULID"),
            ts: Ts::parse(ts).expect("ts"),
            schema: crate::SCHEMA_VERSION,
            target: ObsId::parse(ID_A).expect("ULID"),
            action: ReactionAction::Starred,
        })
    }

    fn model_ref(sha: Option<&str>) -> ModelRef {
        ModelRef {
            provider: "test".into(),
            id: "m".into(),
            prompt_sha256: sha.map(str::to_string),
            calls: vec![],
        }
    }

    fn ingest_obs(id: &str, ts: &str, sha: &str) -> Observation {
        Observation::Ingest(Ingest {
            id: ObsId::parse(id).expect("ULID"),
            ts: Ts::parse(ts).expect("ts"),
            schema: crate::SCHEMA_VERSION,
            source: Source {
                kind: Kind::Image,
                sha256: "0".into(),
                origin: "file:///x.jpg".into(),
                bytes: 1,
                mime: "image/jpeg".into(),
            },
            machine: Machine {
                prose: "산문".into(),
                axes: Axes::from_array([0.4; 8]),
                tags: vec![],
                quality: Quality::Full,
                prompt_sha256: sha.into(),
            },
            min_dist: None,
            surprisal: 1.0,
            model: model_ref(Some(sha)),
            supersedes: None,
        })
    }

    fn delta_obs(id: &str, ts: &str, sha: Option<&str>) -> Observation {
        Observation::SoulDelta(SoulDelta {
            id: ObsId::parse(id).expect("ULID"),
            ts: Ts::parse(ts).expect("ts"),
            schema: crate::SCHEMA_VERSION,
            window: Window {
                from: ObsId::parse(ID_A).expect("ULID"),
                to: ObsId::parse(ID_A).expect("ULID"),
            },
            blocks: BTreeMap::new(),
            axis_delta: AxisDelta::new(),
            morphology_delta: None,
            cites: vec![],
            rationale: "제안".into(),
            model: model_ref(sha),
        })
    }

    /// §R11 — 규칙은 `machine.prompt_sha256`과 **`soul_delta.model.prompt_sha256`을 둘 다**
    /// 이름으로 적고 있다. 성찰 프롬프트(`reflect.md`)를 고쳐서 생긴 경계도 나와야 한다.
    #[test]
    fn prompt_boundaries_cover_soul_delta_not_only_ingest() {
        let set = ObsSet::new(vec![
            ingest_obs(ID_A, "2026-06-01T00:00:00.000Z", "describe-1"),
            delta_obs(ID_B, "2026-06-02T00:00:00.000Z", Some("reflect-1")),
            ingest_obs(ID_C, "2026-06-03T00:00:00.000Z", "describe-1"),
            // 여기서 reflect.md 가 바뀌었다. ingest 쪽은 그대로다.
            delta_obs(ID_D, "2026-06-04T00:00:00.000Z", Some("reflect-2")),
            ingest_obs(ID_E, "2026-06-05T00:00:00.000Z", "describe-2"),
        ]);

        let b = set.prompt_boundaries();
        let got: Vec<(&str, &str, &str)> = b
            .iter()
            .map(|x| (x.id.as_str(), x.kind.as_str(), x.sha256.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                (ID_A, "ingest", "describe-1"),
                (ID_B, "soul_delta", "reflect-1"),
                (ID_D, "soul_delta", "reflect-2"),
                (ID_E, "ingest", "describe-2"),
            ],
            "종류별로 따로 훑고 ULID 순으로 합쳐야 한다"
        );
    }

    /// 두 종류는 **서로 다른 프롬프트 파일**을 쓴다. 한 줄로 섞어 훑으면 번갈아 나올 때마다
    /// 가짜 경계가 생긴다 — 그것이 §R11이 막으려는 바로 그 오류다.
    #[test]
    fn interleaved_kinds_do_not_manufacture_boundaries() {
        let set = ObsSet::new(vec![
            ingest_obs(ID_A, "2026-06-01T00:00:00.000Z", "describe-1"),
            delta_obs(ID_B, "2026-06-02T00:00:00.000Z", Some("reflect-1")),
            ingest_obs(ID_C, "2026-06-03T00:00:00.000Z", "describe-1"),
            delta_obs(ID_D, "2026-06-04T00:00:00.000Z", Some("reflect-1")),
        ]);
        assert_eq!(
            set.prompt_boundaries().len(),
            2,
            "프롬프트가 바뀐 적이 없으면 경계는 종류마다 첫 관측 하나씩뿐이다: {:?}",
            set.prompt_boundaries()
        );
    }

    /// `prompt_sha256`이 없는 `soul_delta`는 "프롬프트가 바뀌었다"고 말할 근거가 아니다.
    /// 없는 것을 하나의 값으로 세면 있지도 않은 경계가 생긴다.
    #[test]
    fn a_soul_delta_without_a_sha_makes_no_boundary() {
        let set = ObsSet::new(vec![
            delta_obs(ID_A, "2026-06-01T00:00:00.000Z", None),
            delta_obs(ID_B, "2026-06-02T00:00:00.000Z", Some("reflect-1")),
            delta_obs(ID_C, "2026-06-03T00:00:00.000Z", None),
            delta_obs(ID_D, "2026-06-04T00:00:00.000Z", Some("reflect-1")),
        ]);
        let b = set.prompt_boundaries();
        assert_eq!(b.len(), 1, "{b:?}");
        assert_eq!(b[0].id.as_str(), ID_B);
    }

    /// §R9 — supersede된 ingest는 경계 계산에서도 빠진다 (§12의 모든 계산과 같은 관문).
    #[test]
    fn superseded_ingests_do_not_make_boundaries() {
        let old = ingest_obs(ID_A, "2026-06-01T00:00:00.000Z", "describe-1");
        let mut new = ingest_obs(ID_B, "2026-06-02T00:00:00.000Z", "describe-2");
        if let Observation::Ingest(i) = &mut new {
            i.supersedes = Some(ObsId::parse(ID_A).unwrap());
        }
        let set = ObsSet::new(vec![old, new]);
        let b = set.prompt_boundaries();
        assert_eq!(b.len(), 1, "{b:?}");
        assert_eq!(b[0].sha256, "describe-2");
    }

    #[test]
    fn append_writes_canonical_json_into_month_shard() {
        // §20.6 — 디렉토리는 관측의 `ts` 기준 UTC 월.
        let (store, paths) = temp_store("store-append");
        let obs = reaction(ID_A, "2026-08-13T09:12:33.123Z");

        let path = store.append(&obs).expect("기록");
        assert_eq!(
            path,
            paths
                .observations()
                .join("2026-08")
                .join(format!("{ID_A}.json"))
        );

        // §R6 — 파일 내용은 `to_canonical_json()` 결과 그대로다.
        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(written, obs.to_canonical_json().unwrap());
        assert!(written.ends_with("}\n"));
        assert!(!written.contains('\r'));

        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn append_uses_ts_month_not_ulid_month() {
        // ULID에 박힌 시각이 아니라 `ts`가 샤드를 정한다.
        let (store, paths) = temp_store("store-shard");
        let obs = reaction(ID_A, "2025-12-31T23:59:59.999Z");
        let path = store.append(&obs).expect("기록");
        assert!(path.parent().unwrap().ends_with("2025-12"), "{path:?}");
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn append_rejects_existing_id() {
        // 관측은 불변이다 — 같은 ID를 두 번 쓸 수 없다 (§R2).
        let (store, paths) = temp_store("store-dup");
        let obs = reaction(ID_A, "2026-08-13T09:12:33.123Z");
        store.append(&obs).expect("첫 기록");
        assert!(store.append(&obs).is_err(), "중복 ID는 거부해야 한다");
        assert_eq!(store.count().unwrap(), 1);

        // 월이 달라도(= 경로가 달라도) ID가 같으면 거부한다.
        let same_id_other_month = reaction(ID_A, "2026-09-01T00:00:00.000Z");
        assert!(store.append(&same_id_other_month).is_err());
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn append_rejects_invalid_observation() {
        let (store, paths) = temp_store("store-invalid");
        // §6.3 — verdict=yes 인데 prose가 있으면 기록 자체를 막는다 (T7).
        let bad = Observation::Reading(Reading {
            id: ObsId::parse(ID_A).unwrap(),
            ts: Ts::parse("2026-08-13T09:12:33.123Z").unwrap(),
            schema: crate::SCHEMA_VERSION,
            layer: Layer::Sensory,
            target: ObsId::parse(ID_B).unwrap(),
            verdict: Verdict::Yes,
            prose: Some("x".into()),
            divergence: None,
        });
        assert!(store.append(&bad).is_err());
        assert!(
            !paths.observations().join("2026-08").exists(),
            "파일을 만들면 안 된다"
        );
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn replay_order_is_ulid_ascending_across_months() {
        // 재생 순서는 ULID 사전순이며 디렉토리 경로는 순서에 영향을 주지 않는다 (§20.6).
        let (store, paths) = temp_store("store-order");
        store
            .append(&reaction(ID_C, "2026-01-05T00:00:00.000Z"))
            .unwrap();
        store
            .append(&reaction(ID_A, "2026-09-05T00:00:00.000Z"))
            .unwrap();
        store
            .append(&reaction(ID_B, "2025-12-05T00:00:00.000Z"))
            .unwrap();

        let ids: Vec<String> = store.ids().unwrap().iter().map(|i| i.to_string()).collect();
        assert_eq!(
            ids,
            vec![ID_A.to_string(), ID_B.to_string(), ID_C.to_string()]
        );
        let all: Vec<String> = store
            .all()
            .unwrap()
            .iter()
            .map(|o| o.id().to_string())
            .collect();
        assert_eq!(all, ids);
        assert_eq!(store.count().unwrap(), 3);
        assert_eq!(store.load_set().unwrap().len(), 3);
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn read_roundtrips_and_missing_id_is_not_found() {
        let (store, paths) = temp_store("store-read");
        let obs = reaction(ID_A, "2026-08-13T09:12:33.123Z");
        store.append(&obs).unwrap();

        let id_a = ObsId::parse(ID_A).unwrap();
        let id_b = ObsId::parse(ID_B).unwrap();
        assert_eq!(store.read(&id_a).unwrap(), obs);
        assert!(store.exists(&id_a));
        assert!(!store.exists(&id_b));
        match store.read(&id_b) {
            Err(SoulError::ObsNotFound(s)) => assert_eq!(s, ID_B),
            other => panic!(
                "ObsNotFound 를 기대했다: {:?}",
                other.map(|o| o.id().clone())
            ),
        }
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn empty_store_scans_to_nothing() {
        // 최초 실행 — observations/ 가 비어 있거나 아예 없어도 에러가 아니다 (§3).
        let root =
            std::env::temp_dir().join(format!("tasty-soul-store-empty-{}", crate::ids::new_id()));
        let store = Store::new(Paths::at(&root));
        assert_eq!(store.count().unwrap(), 0);
        assert!(store.all().unwrap().is_empty());
        assert!(!store.exists(&ObsId::parse(ID_A).unwrap()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_skips_non_observation_files_and_invalidate_rescans() {
        let (store, paths) = temp_store("store-scan");
        store
            .append(&reaction(ID_A, "2026-08-13T09:12:33.123Z"))
            .unwrap();

        let month = paths.observations().join("2026-08");
        std::fs::write(month.join("메모.txt"), "x").unwrap();
        std::fs::write(month.join("temp-not-a-ulid.json"), "{}").unwrap();
        std::fs::write(paths.observations().join("느슨한파일.json"), "{}").unwrap();

        // 외부에서 추가된 관측은 invalidate 이후에 보인다.
        let extra = reaction(ID_B, "2026-08-14T00:00:00.000Z");
        std::fs::write(
            month.join(format!("{ID_B}.json")),
            extra.to_canonical_json().unwrap(),
        )
        .unwrap();
        assert_eq!(store.count().unwrap(), 1, "캐시된 인덱스는 그대로다");
        store.invalidate();
        assert_eq!(store.count().unwrap(), 2, "ULID 파일만 새로 잡힌다");
        assert_eq!(store.read(&ObsId::parse(ID_B).unwrap()).unwrap(), extra);

        let _ = std::fs::remove_dir_all(&paths.root);
    }
}
