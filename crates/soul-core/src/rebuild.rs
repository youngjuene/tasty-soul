//! 재빌드 — **재호출이 아니라 재생** (§R2 · §14).
//!
//! | 명령 | derived.sqlite | SOUL.md | 커밋 |
//! |---|---|---|---|
//! | `soul render` | 읽기만 | 재작성 | `render <T_ref>` |
//! | `soul rebuild` | 관측 재생으로 갱신 | 재작성 | `rebuild <n>` |
//! | `soul rebuild --from-scratch` | **삭제 후 전량 재구축** | 재작성 | `rebuild <n>` |
//!
//! `--offline`은 세 형태 모두에 붙일 수 있으며, 임베딩 캐시 미스 시 **에러 종료**시킨다 (T28).
//!
//! ## `soul:human` (§R2 예외)
//!
//! 이 블록은 어떤 관측도 만들지 않으므로 로그만으로는 복원할 수 없다.
//! 재빌드는 **기존 `SOUL.md` 파일에서 이 블록을 그대로 읽어 이월한다.**
//! 이월할 블록을 찾지 못해 **빈 채로 렌더할 때마다 경고를 출력한다** (T4b).
//! 파일이 없을 때만이 아니다 — 0바이트로 잘린 파일도, 블록이 통째로 없는 파일도
//! 같은 손실이다. `carry_over_human` 주석 참고 (§18-5).

use crate::config::Config;
use crate::db::embed_cache::cache_key;
use crate::db::Db;
use crate::derived::Derived;
use crate::error::{Result, SoulError};
use crate::ids::ObsId;
use crate::obs::{ObsSet, Observation, Store};
use crate::paths::Paths;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

/// 임베딩 캐시 키의 `provider` 성분 (§R3).
///
/// `config.toml`에는 제공자 항목이 없다 — §D3이 임베딩 제공자를 OpenAI 하나로 못박기
/// 때문이다. `api.base_url`에서 유도하지 **않는다**: 프록시로 바꿨다는 이유로 워밍된
/// 캐시가 통째로 미스가 되면 `--offline` 재빌드가 까닭 없이 실패한다.
/// `soul-net::embed::Embedder::provider`는 반드시 이 값과 같아야 한다.
pub const EMBED_PROVIDER: &str = "openai";

/// `soul:neg` 프로필 블록의 id (§8.2 템플릿).
const PROFILE_BLOCK: &str = "profile";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RebuildOptions {
    /// derived.sqlite를 삭제하고 전량 재구축한다. 임베딩 캐시 테이블은 보존한다 (T2).
    pub from_scratch: bool,
    /// 임베딩 캐시 미스 시 에러 종료 (§R3).
    pub offline: bool,
}

// 두 필드 모두 false지만 `#[derive(Default)]`로 바꾸지 않는다.
// 기본값이 "네트워크를 쓰는 증분 재빌드"라는 사실을 코드에 남겨 둔다.
#[allow(clippy::derivable_impls)]
impl Default for RebuildOptions {
    fn default() -> Self {
        RebuildOptions {
            from_scratch: false,
            offline: false,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct RebuildReport {
    pub observations: usize,
    pub soul_md_changed: bool,
    /// 이월할 `soul:human` 블록이 없어 빈 채로 렌더된 경우 (T4b).
    /// 파일 없음 · 0바이트 · 블록 없음이 모두 여기 해당한다 (§18-5).
    pub warnings: Vec<String>,
    pub commit: Option<String>,
}

/// 관측을 ULID 순으로 재생해 파생값을 만든다. **API를 호출하지 않는다.**
/// 임베딩이 필요하면 `missing_embeddings`로 돌려주고, 호출자(soul-pipeline)가 채운 뒤
/// 다시 부른다. 이 구조 덕분에 `soul-core`가 네트워크를 몰라도 된다.
pub fn replay(paths: &Paths, offline: bool) -> Result<(Derived, Vec<String>)> {
    let set = Store::new(paths.clone()).load_set()?;
    let cfg = Config::load(&paths.config_toml())?;

    // 임베딩 캐시는 **텍스트로** 조회한다 (§R3의 캐시 키). `obs_vec`/`critique_vec`으로
    // 조회하면 `--from-scratch`가 그 표들을 비운 직후(T2) 캐시가 워밍되어 있는데도
    // 전부 미스가 되어 T1이 깨진다.
    let db = if paths.derived_db().is_file() {
        Some(Db::open(&paths.derived_db())?)
    } else {
        // 캐시 파일이 없으면 전부 미스다. 읽기 경로에서 새 파일을 만들지 않는다.
        None
    };

    let (embeds, missing) = embedding_lookup(&set, db.as_ref(), &cfg)?;

    // T28 — 오프라인인데 미스가 있으면 파생값을 만들지 않고 즉시 멈춘다.
    if offline && !missing.is_empty() {
        let head: String = missing[0].chars().take(40).collect();
        return Err(SoulError::EmbedCacheMiss(format!(
            "{}건. 첫 항목: \"{head}…\"",
            missing.len()
        )));
    }

    let derived = crate::derived::compute(&set, &embeds, cfg.local.silhouette_max_samples);
    Ok((derived, missing))
}

/// 텍스트 → 벡터 표와, 캐시에 없던 텍스트 목록.
pub type EmbeddingTable = (BTreeMap<String, Vec<f32>>, Vec<String>);

/// 관측 집합이 필요로 하는 텍스트 → 벡터 표를 **임베딩 캐시에서 텍스트로** 조회한다.
///
/// 반환값의 두 번째 요소는 캐시에 없던 텍스트다. **네트워크를 쓰지 않는다.**
///
/// # 왜 `obs_vec`/`critique_vec`을 쓰지 않는가
///
/// 그 표들은 관측 **ID** 색인이고 `--from-scratch`가 비울 수 있다(T2). 캐시가 워밍된
/// 상태인데도 전부 미스가 되어 T1이 깨진다. 더 중요하게는, ID 색인은 `reading.prose`
/// 벡터를 담지 않아도 되는 곳인데(§12.7 불변식) 거기서 조회하면 **담아야만** 하게 된다.
///
/// **파생값을 계산하는 모든 경로가 이 함수를 거쳐야 한다.** 다른 방법으로 표를 만들면
/// 같은 로그에서 서로 다른 `Derived`가 나오고, 그것은 §R2가 금지하는 바로 그 어긋남이다
/// (대시보드와 `SOUL.md`가 다른 숫자를 말한다).
pub fn embedding_lookup(set: &ObsSet, db: Option<&Db>, cfg: &Config) -> Result<EmbeddingTable> {
    let mut embeds: BTreeMap<String, Vec<f32>> = BTreeMap::new();
    let mut missing: Vec<String> = Vec::new();
    for text in wanted_embeddings(set) {
        let hit = match db {
            Some(db) => {
                let key = cache_key(EMBED_PROVIDER, &cfg.embed.model, cfg.embed.dims, &text);
                db.embed_get(&key)?
            }
            None => None,
        };
        match hit {
            Some(v) => {
                embeds.insert(text, v);
            }
            None => missing.push(text),
        }
    }
    Ok((embeds, missing))
}

/// `SOUL.md`를 재렌더하고 커밋한다 (§R8의 재렌더 계기).
pub fn render_soul_md(
    paths: &Paths,
    derived: &Derived,
    commit_message: &str,
) -> Result<RebuildReport> {
    // 1. 쓰기 락 (§R7 · §8.4 1단계). 실패하면 아무것도 쓰지 않는다.
    let soul_dir = paths.soul();
    std::fs::create_dir_all(&soul_dir)?;
    let _lock = crate::lock::WriteLock::acquire(&soul_dir)?;

    let mut report = RebuildReport::default();

    // 2. `soul:human`은 재생 대상이 아니다 (§R2 예외). 기존 파일에서 그대로 이월한다.
    let md_path = paths.soul_md();
    let existing = if md_path.is_file() {
        Some(std::fs::read_to_string(&md_path)?)
    } else {
        None
    };
    let (human, warnings) = carry_over_human(&md_path, existing.as_deref())?;
    report.warnings = warnings;

    // 3. `soul:neg id=profile`은 관측 재생 결과다.
    let set = Store::new(paths.clone()).load_set()?;
    let (profile_text, profile_rev) = profile_from_observations(&set);

    // 4. 대표 사례는 sensory 층의 것을 쓴다 (docs/OPEN-DECISIONS.md #17).
    let examples: Vec<ObsId> = derived
        .coherence_sensory
        .as_ref()
        .map(|c| c.examples.clone())
        .unwrap_or_default();

    let text = crate::soulmd::render(&crate::soulmd::RenderInput {
        derived,
        profile_text: &profile_text,
        profile_rev,
        human_text: &human,
        divergence_examples: &examples,
    });

    report.observations = set.len();
    report.soul_md_changed = existing.as_deref() != Some(text.as_str());

    // 5. 쓰고 커밋한다. 쓰기 한 번 = 커밋 하나 (§R8).
    //    **원자적으로** 쓴다 — 제자리 덮어쓰기는 파일을 먼저 0바이트로 자르고,
    //    그 창에서 죽으면 2단계에서 이월한 `soul:human`이 사라진다 (§18-5).
    crate::git::ensure_repo(&soul_dir)?;
    crate::soulmd::save::write_soul_md_atomic(&md_path, &text)?;
    report.commit = crate::git::commit_paths(&soul_dir, &[Path::new("SOUL.md")], commit_message)?;

    Ok(report)
}

/// 기존 `SOUL.md`에서 `soul:human` 본문을 이월한다 (§R2 예외).
///
/// ## 경고 조건은 "파일이 있는가"가 아니라 "빈 채로 렌더하는가"이다 (§18-5 · T4b)
///
/// | 상태 | 경고 |
/// |---|---|
/// | 파일 없음 | **한다** |
/// | 파일은 있는데 `soul:human` 블록이 없음 (0바이트 포함) | **한다** |
/// | 블록은 있는데 본문이 비어 있음 | 하지 않는다 — 원래 비어 있던 정상 상태다 |
///
/// 파일 유무로 판단하면 두 곳에서 조용히 침묵한다. 하나는 비원자적 쓰기가 중단되어
/// 생긴 **0바이트 `SOUL.md`**이고(그래서 위의 `write_soul_md_atomic`이 있다), 다른 하나는
/// 블록이 통째로 날아간 파일이다. 둘 다 "파일은 존재한다". 게다가
/// `soul-pipeline::App::open_or_init`이 `SOUL.md`가 없으면 스켈레톤을 자동 생성하므로
/// **"파일 없음" 경고는 실사용에서 사실상 영원히 뜨지 않는다.**
///
/// `soul:human`은 어떤 관측도 만들지 않는 유일한 1차 자료다 — 로그에 사본이 없으므로
/// 조용히 비우면 사용자가 직접 쓴 문장이 그대로 사라진다. 그래서 경고에 복구 방법
/// (`git log -p SOUL.md`, T4c)을 반드시 함께 적는다. 사라졌다는 사실만 알려 주는 경고는
/// 사용자가 할 수 있는 일이 없다.
///
/// 반대로 빈 블록에까지 경고하면 최초 실행부터 매 렌더마다 경고가 떠서 진짜 경고가
/// 묻힌다. 그래서 "블록의 유무"로 가른다.
fn carry_over_human(md_path: &Path, existing: Option<&str>) -> Result<(String, Vec<String>)> {
    // 파싱 실패는 그대로 올린다 — §15는 "파일을 쓰지 않고 알린다"이다.
    let doc = match existing {
        Some(text) => Some(crate::soulmd::parse(text)?),
        None => None,
    };

    // 블록이 있으면 본문이 비어 있어도 그대로 이월한다(경고 없음).
    if let Some(body) = doc.as_ref().and_then(|d| d.human_body()) {
        return Ok((body.to_string(), Vec::new()));
    }

    // 두 사유를 구별해 적는다. 사용자가 파일을 지운 것과 파일이 잘린 것은
    // 다음에 할 일이 다르다(전자는 복구, 후자는 디스크·강제 종료를 의심한다).
    let cause = match existing {
        None => format!("{} 가 없습니다", md_path.display()),
        // 0바이트로 잘린 파일도 여기로 온다. 파일이 있다는 사실은 위로가 되지 않는다.
        Some(_) => format!("{} 에 soul:human 블록이 없습니다", md_path.display()),
    };
    Ok((
        String::new(),
        vec![format!(
            "{cause}. soul:human 을 빈 채로 렌더했습니다 — 이 블록만은 관측 로그로 \
             복원되지 않습니다. 이전 내용은 `git log -p SOUL.md` 로 복구할 수 있습니다 \
             (§R2, T4b·T4c)"
        )],
    ))
}

/// 임베딩이 필요한 텍스트를 ULID 순·중복 제거해서 모은다.
///
/// | 출처 | §12.7 공간 |
/// |---|---|
/// | 활성 `ingest.machine.prose` (§R9) | Object |
/// | `context.critique` | Critique |
/// | `reading.prose` (layer=sensory) | Object |
/// | `reading.prose` (layer=cultural) | Critique |
///
/// 공간은 벡터를 어느 표에 **연결**할지를 정할 뿐이고, `embed_cache`의 키는
/// 텍스트다. 따라서 조회 자체는 공간과 무관하다.
fn wanted_embeddings(set: &ObsSet) -> Vec<String> {
    let dead = set.superseded_ids();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for o in set.as_slice() {
        let text: Option<&str> = match o {
            Observation::Ingest(i) if !dead.contains(&i.id) => Some(i.machine.prose.as_str()),
            Observation::Context(c) => Some(c.critique.as_str()),
            Observation::Reading(r) => r.prose.as_deref(),
            _ => None,
        };
        if let Some(t) = text {
            if !t.is_empty() && seen.insert(t) {
                out.push(t.to_string());
            }
        }
    }
    out
}

/// `soul:neg id=profile`의 본문과 `rev` (§8.1 · §8.3 규칙 4).
///
/// 본문은 profile 블록을 마지막으로 건드린 관측의 `to_text`이고,
/// `rev`는 그런 관측의 누적 개수다. 하나도 없으면 `("", 0)`.
fn profile_from_observations(set: &ObsSet) -> (String, u32) {
    let mut text = String::new();
    let mut rev: u32 = 0;
    // `ObsSet`은 ULID 오름차순이므로 그냥 훑으면 마지막 것이 남는다 (§R2).
    for o in set.as_slice() {
        let to_text = match o {
            Observation::ProfileEdit(e) if e.block == PROFILE_BLOCK => Some(e.to_text.clone()),
            Observation::SoulDelta(d) => d.blocks.get(PROFILE_BLOCK).map(|b| b.to_text.clone()),
            _ => None,
        };
        if let Some(t) = to_text {
            text = t;
            rev = rev.saturating_add(1);
        }
    }
    (text, rev)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obs::model::*;
    use crate::time::Ts;
    use crate::SCHEMA_VERSION;

    fn temp_paths() -> Paths {
        let root = std::env::temp_dir()
            .join("tasty-soul-rebuild-test")
            .join(crate::ids::new_id().to_string());
        let p = Paths::at(root);
        p.ensure_dirs().unwrap();
        p
    }

    /// 오름차순이 보장되는 결정론적 ULID.
    fn id(n: u32) -> ObsId {
        ObsId::parse(&format!("01J8XQZK3M7P4RSTVWXYZ{n:05}")).unwrap()
    }

    fn ts(day: u32) -> Ts {
        Ts::parse(&format!("2026-08-{day:02}T09:12:33.123Z")).unwrap()
    }

    fn model_ref() -> ModelRef {
        ModelRef {
            provider: "openai".into(),
            id: "gpt-x".into(),
            prompt_sha256: None,
            calls: vec![],
        }
    }

    fn ingest(n: u32, prose: &str, supersedes: Option<ObsId>) -> Observation {
        Observation::Ingest(Ingest {
            id: id(n),
            ts: ts(n),
            schema: SCHEMA_VERSION,
            source: Source {
                kind: Kind::Image,
                sha256: format!("{n:064}"),
                origin: format!("file:///tmp/{n}.jpg"),
                bytes: 100,
                mime: "image/jpeg".into(),
            },
            machine: Machine {
                prose: prose.into(),
                axes: Axes::ZERO,
                tags: vec![],
                quality: Quality::Full,
                prompt_sha256: "9f2c1a".into(),
            },
            min_dist: None,
            surprisal: 0.0,
            model: model_ref(),
            supersedes,
        })
    }

    fn reading(n: u32, target: ObsId, layer: Layer, prose: Option<&str>) -> Observation {
        Observation::Reading(Reading {
            id: id(n),
            ts: ts(n),
            schema: SCHEMA_VERSION,
            layer,
            target,
            verdict: if prose.is_some() {
                Verdict::No
            } else {
                Verdict::Yes
            },
            prose: prose.map(|s| s.to_string()),
            divergence: None,
        })
    }

    fn context(n: u32, target: ObsId, critique: &str) -> Observation {
        Observation::Context(ContextObs {
            id: id(n),
            ts: ts(n),
            schema: SCHEMA_VERSION,
            target,
            critique: critique.into(),
            lineage: vec![],
            queries: vec![],
            sources: vec![],
            grounded: false,
            model: model_ref(),
        })
    }

    fn soul_delta(n: u32, block: &str, to_text: &str) -> Observation {
        let mut blocks = BTreeMap::new();
        blocks.insert(
            block.to_string(),
            BlockDelta {
                from_hash: "a3f91c".into(),
                to_text: to_text.into(),
            },
        );
        Observation::SoulDelta(SoulDelta {
            id: id(n),
            ts: ts(n),
            schema: SCHEMA_VERSION,
            window: Window {
                from: id(1),
                to: id(n),
            },
            blocks,
            axis_delta: AxisDelta::new(),
            morphology_delta: None,
            cites: vec![],
            rationale: "성찰 결과".into(),
            model: model_ref(),
        })
    }

    fn profile_edit(n: u32, block: &str, to_text: &str) -> Observation {
        Observation::ProfileEdit(ProfileEdit {
            id: id(n),
            ts: ts(n),
            schema: SCHEMA_VERSION,
            block: block.into(),
            from_hash: "a3f91c".into(),
            to_text: to_text.into(),
            author: "user".into(),
        })
    }

    fn write_obs(paths: &Paths, o: &Observation) {
        let path = paths.observation_file(o.ts(), o.id());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, o.to_canonical_json().unwrap()).unwrap();
    }

    // ───────────────────────────────────────────── profile 블록 재생

    #[test]
    fn profile_is_empty_without_observations() {
        assert_eq!(
            profile_from_observations(&ObsSet::default()),
            (String::new(), 0)
        );
    }

    #[test]
    fn profile_takes_last_to_text_and_counts_rev() {
        let set = ObsSet::new(vec![
            soul_delta(1, PROFILE_BLOCK, "첫 성찰 결과"),
            profile_edit(2, PROFILE_BLOCK, "사람이 고쳐 쓴 문장"),
            soul_delta(3, PROFILE_BLOCK, "두 번째 성찰 결과"),
        ]);
        let (text, rev) = profile_from_observations(&set);
        assert_eq!(text, "두 번째 성찰 결과");
        assert_eq!(rev, 3, "profile_edit + soul_delta 누적 수");
    }

    #[test]
    fn profile_ignores_other_blocks() {
        let set = ObsSet::new(vec![
            soul_delta(1, PROFILE_BLOCK, "프로필 본문"),
            soul_delta(2, "morphology", "다른 블록"),
            profile_edit(3, "notes", "다른 블록"),
        ]);
        let (text, rev) = profile_from_observations(&set);
        assert_eq!(text, "프로필 본문");
        assert_eq!(rev, 1);
    }

    // ───────────────────────────────────────────── 임베딩 대상 수집

    #[test]
    fn wanted_embeddings_covers_both_spaces_in_ulid_order() {
        let set = ObsSet::new(vec![
            ingest(1, "차갑고 정돈된 실내", None),
            reading(2, id(1), Layer::Sensory, Some("사람이 나간 자리 같다")),
            context(
                3,
                id(1),
                "이 사진은 1970년대 뉴토포그래픽스의 어법을 따른다",
            ),
            reading(
                4,
                id(3),
                Layer::Cultural,
                Some("계보보다 습도가 먼저 읽힌다"),
            ),
            reading(5, id(1), Layer::Sensory, None), // verdict=yes → prose 없음
        ]);
        assert_eq!(
            wanted_embeddings(&set),
            vec![
                "차갑고 정돈된 실내".to_string(),
                "사람이 나간 자리 같다".to_string(),
                "이 사진은 1970년대 뉴토포그래픽스의 어법을 따른다".to_string(),
                "계보보다 습도가 먼저 읽힌다".to_string(),
            ]
        );
    }

    /// §R9 — supersede된 ingest의 `machine.prose`는 임베딩 대상이 아니다.
    #[test]
    fn wanted_embeddings_skips_superseded_ingests() {
        let set = ObsSet::new(vec![
            ingest(1, "낡은 서술", None),
            ingest(2, "다시 쓴 서술", Some(id(1))),
        ]);
        assert_eq!(wanted_embeddings(&set), vec!["다시 쓴 서술".to_string()]);
    }

    #[test]
    fn wanted_embeddings_dedupes_identical_text() {
        let set = ObsSet::new(vec![
            ingest(1, "같은 서술", None),
            ingest(2, "같은 서술", None),
        ]);
        assert_eq!(wanted_embeddings(&set), vec!["같은 서술".to_string()]);
    }

    // ───────────────────────────────────────────── soul:human 이월 (T4b)

    #[test]
    fn missing_soul_md_warns_and_leaves_human_empty() {
        let (human, warnings) = carry_over_human(Path::new("/x/SOUL.md"), None).unwrap();
        assert!(human.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("soul:human"), "{}", warnings[0]);
        // T4c — 사용자가 실제로 되찾을 수 있는 방법을 경고에 적는다.
        assert!(
            warnings[0].contains("git log -p SOUL.md"),
            "{}",
            warnings[0]
        );
    }

    /// §18-5 — 0바이트 `SOUL.md`는 "파일이 있다"가 아니라 "빈 채로 렌더한다"이다.
    ///
    /// 비원자적 쓰기가 중간에 끊기면 이 상태가 만들어진다. 파일이 존재한다는 이유로
    /// 침묵하면 사람이 쓴 문장이 조용히 사라진 사실이 아무 데도 남지 않는다.
    #[test]
    fn zero_byte_soul_md_warns() {
        let (human, warnings) = carry_over_human(Path::new("/x/SOUL.md"), Some("")).unwrap();
        assert!(human.is_empty());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("soul:human"), "{}", warnings[0]);
        assert!(
            warnings[0].contains("git log -p SOUL.md"),
            "{}",
            warnings[0]
        );
    }

    /// §18-5 — 파일은 멀쩡한데 `soul:human` 블록만 없는 경우도 마찬가지다.
    #[test]
    fn soul_md_without_human_block_warns() {
        let text = "# SOUL\n\n<!-- soul:gen id=header -->\n관측 0\n<!-- /soul:gen -->\n";
        let (human, warnings) = carry_over_human(Path::new("/x/SOUL.md"), Some(text)).unwrap();
        assert!(human.is_empty());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("soul:human"), "{}", warnings[0]);
        assert!(
            warnings[0].contains("git log -p SOUL.md"),
            "{}",
            warnings[0]
        );
    }

    /// 오탐 방지 — 블록이 **있는데** 본문이 비어 있는 것은 정상 상태다 (스켈레톤이 그렇다).
    /// 여기서 경고하면 최초 실행부터 매 렌더마다 경고가 떠서 진짜 경고가 묻힌다.
    #[test]
    fn empty_human_block_does_not_warn() {
        let text = "# SOUL\n\n<!-- soul:human -->\n\n<!-- /soul:human -->\n";
        let (human, warnings) = carry_over_human(Path::new("/x/SOUL.md"), Some(text)).unwrap();
        assert!(human.trim().is_empty(), "{human:?}");
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    /// 회귀 방지 — 사람이 쓴 본문은 경고 없이 그대로 이월된다.
    #[test]
    fn nonempty_human_block_is_carried_over_without_warning() {
        let text = "# SOUL\n\n<!-- soul:human -->\n습도라는 말을 쓰기 시작한 건 3월부터다.\n<!-- /soul:human -->\n";
        let (human, warnings) = carry_over_human(Path::new("/x/SOUL.md"), Some(text)).unwrap();
        assert!(
            human.contains("습도라는 말을 쓰기 시작한 건 3월부터다."),
            "{human:?}"
        );
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    // ───────────────────────────────────────────── T28

    /// T28 — `--offline`인데 임베딩 캐시에 없으면 파생값을 만들지 않고 에러다.
    #[test]
    fn offline_replay_errors_on_embed_cache_miss() {
        let paths = temp_paths();
        write_obs(
            &paths,
            &ingest(1, "차갑고 정돈된, 사람이 방금 지워진 실내", None),
        );
        assert!(
            !paths.derived_db().exists(),
            "캐시 파일이 없는 상태여야 한다"
        );

        let err = replay(&paths, true).unwrap_err();
        match err {
            SoulError::EmbedCacheMiss(msg) => {
                assert!(msg.contains('1'), "미스 건수를 알려야 한다: {msg}");
            }
            other => panic!("EmbedCacheMiss 여야 한다: {other:?}"),
        }
    }

    /// 관측이 없으면 임베딩도 필요 없으므로 오프라인이어도 실패하지 않는다.
    #[test]
    fn offline_replay_with_no_observations_succeeds() {
        let paths = temp_paths();
        let (derived, missing) = replay(&paths, true).unwrap();
        assert!(missing.is_empty());
        assert_eq!(derived.observation_count, 0);
        assert!(derived.t_ref.is_none());
    }

    /// 캐시가 워밍되어 있으면 `--offline` 재생이 성공한다 (§R3의 반대 방향).
    #[test]
    fn warm_cache_makes_offline_replay_succeed() {
        let paths = temp_paths();
        let prose = "차갑고 정돈된, 사람이 방금 지워진 실내";
        write_obs(&paths, &ingest(1, prose, None));

        let cfg = Config::default();
        {
            let db = Db::open(&paths.derived_db()).unwrap();
            let key = cache_key(EMBED_PROVIDER, &cfg.embed.model, cfg.embed.dims, prose);
            db.embed_put(&key, cfg.embed.dims, &vec![0.25_f32; cfg.embed.dims])
                .unwrap();
        }

        let (derived, missing) = replay(&paths, true).unwrap();
        assert!(missing.is_empty(), "{missing:?}");
        assert_eq!(derived.observation_count, 1);
        assert_eq!(derived.t_ref, Some(ts(1)));
    }

    /// 온라인 재생은 미스를 에러가 아니라 **목록으로** 돌려준다 — 호출자가 채운다.
    #[test]
    fn online_replay_returns_misses_instead_of_failing() {
        let paths = temp_paths();
        let prose = "젖은 아스팔트 위의 신호등";
        write_obs(&paths, &ingest(1, prose, None));

        let (derived, missing) = replay(&paths, false).unwrap();
        assert_eq!(missing, vec![prose.to_string()]);
        assert_eq!(derived.observation_count, 1);
    }

    // ───────────────────────────────────────────── T4b (전체 경로)

    /// T4b — `SOUL.md`가 없는 상태에서 재렌더하면 경고가 붙는다.
    #[test]
    fn render_soul_md_without_file_reports_warning() {
        let paths = temp_paths();
        write_obs(
            &paths,
            &soul_delta(1, PROFILE_BLOCK, "인공물보다 방치된 것을 고른다"),
        );
        assert!(!paths.soul_md().exists());

        let report = render_soul_md(&paths, &Derived::default(), "rebuild 1").unwrap();

        assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
        assert!(report.warnings[0].contains("soul:human"));
        assert!(report.soul_md_changed);
        assert_eq!(report.observations, 1);

        let text = std::fs::read_to_string(paths.soul_md()).unwrap();
        assert!(text.contains("인공물보다 방치된 것을 고른다"), "{text}");
        assert!(!text.starts_with('\u{feff}'), "BOM 없음");
        assert!(!text.contains('\r'), "LF만");
    }

    /// §18-5 — 잘린 `SOUL.md`(0바이트)로 재렌더해도 경고가 붙는다 (전체 경로).
    #[test]
    fn render_soul_md_with_zero_byte_file_reports_warning() {
        let paths = temp_paths();
        std::fs::create_dir_all(paths.soul()).unwrap();
        // 비원자적 쓰기가 중단된 직후의 상태를 그대로 만든다.
        std::fs::write(paths.soul_md(), b"").unwrap();
        assert_eq!(std::fs::metadata(paths.soul_md()).unwrap().len(), 0);

        let report = render_soul_md(&paths, &Derived::default(), "render 1").unwrap();

        assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
        assert!(
            report.warnings[0].contains("soul:human"),
            "{:?}",
            report.warnings
        );
        assert!(
            report.warnings[0].contains("git log -p SOUL.md"),
            "{:?}",
            report.warnings
        );
    }

    /// §18-5 — `SOUL.md`는 **원자적으로** 쓴다: 임시 파일에 다 쓰고 `rename` 한다.
    ///
    /// 제자리 덮어쓰기(`fs::write`)는 열자마자 파일을 0바이트로 자른다. 그 창에서
    /// 프로세스가 죽으면 사람이 쓴 `soul:human`이 사라진 0바이트 파일만 남는다.
    /// 하드 링크로 **옛 inode**를 붙잡아 두면 두 방식이 구별된다 —
    /// `rename`은 디렉토리 엔트리만 갈아끼우므로 옛 inode의 내용이 살아 있고,
    /// 제자리 덮어쓰기는 같은 inode를 잘라 쓰므로 링크 쪽 내용까지 바뀐다.
    #[cfg(unix)]
    #[test]
    fn render_soul_md_replaces_the_file_atomically() {
        let paths = temp_paths();
        std::fs::create_dir_all(paths.soul()).unwrap();
        let mine = "습도라는 말을 쓰기 시작한 건 3월부터다.";
        let before = format!("# SOUL\n\n<!-- soul:human -->\n{mine}\n<!-- /soul:human -->\n");
        std::fs::write(paths.soul_md(), &before).unwrap();
        let old_inode = paths.soul().join("old-inode");
        std::fs::hard_link(paths.soul_md(), &old_inode).unwrap();

        render_soul_md(&paths, &Derived::default(), "render 1").unwrap();

        assert_eq!(
            std::fs::read_to_string(&old_inode).unwrap(),
            before,
            "제자리 덮어쓰기는 0바이트 창을 만든다 — 임시 파일 + rename 이어야 한다"
        );
        assert!(std::fs::read_to_string(paths.soul_md())
            .unwrap()
            .contains(mine));
        // 임시 파일이 남으면 `git status`가 더러워지고 다음 커밋에 딸려 들어간다 (T20).
        let leftovers: Vec<String> = std::fs::read_dir(paths.soul())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    /// 기존 파일의 `soul:human`은 재렌더 후에도 그대로 남는다 (§R2 · T4).
    #[test]
    fn render_soul_md_carries_over_human_block() {
        let paths = temp_paths();
        let first = render_soul_md(&paths, &Derived::default(), "render 1").unwrap();
        assert_eq!(first.warnings.len(), 1);

        let text = std::fs::read_to_string(paths.soul_md()).unwrap();
        let mine = "습도라는 말을 쓰기 시작한 건 3월부터다.";
        let edited = text.replace(
            "<!-- soul:human -->\n",
            &format!("<!-- soul:human -->\n{mine}\n"),
        );
        assert_ne!(edited, text, "§8.2 템플릿에 soul:human 마커가 있어야 한다");
        std::fs::write(paths.soul_md(), &edited).unwrap();

        let second = render_soul_md(&paths, &Derived::default(), "render 2").unwrap();
        assert!(second.warnings.is_empty(), "{:?}", second.warnings);
        let after = std::fs::read_to_string(paths.soul_md()).unwrap();
        assert!(
            after.contains(mine),
            "soul:human이 이월되어야 한다:\n{after}"
        );
    }
}
