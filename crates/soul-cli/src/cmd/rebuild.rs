//! `soul render` · `soul rebuild` (§14 표 · §R2 · §R8).
//!
//! | 명령 | derived.sqlite | SOUL.md | 커밋 |
//! |---|---|---|---|
//! | `soul render` | **읽기만** | 재작성 | `render <T_ref>` |
//! | `soul rebuild` | 관측 재생으로 갱신 | 재작성 | `rebuild <n>` |
//! | `soul rebuild --from-scratch` | **삭제 후 전량 재구축** | 재작성 | `rebuild <n>` |
//!
//! 셋의 범위를 섞으면 T1이 실패한다. 이 파일에서 지키는 구분은 둘이다.
//!
//! - `render`는 `Db`를 **쓰기로 열지 않는다.** 임베딩 캐시 미스가 있어도 채우지 않고
//!   경고만 낸다 — 채우는 것은 네트워크이고, `render`에는 네트워크가 없다.
//! - `--from-scratch`는 `clear_derived()`만 부른다. `embed_cache`·`obs_vec`·
//!   `critique_vec`·`critique_queue`는 **보존된다** (T2). 파일을 지우면 T2가 깨진다.

use anyhow::{bail, Context, Result};
use soul_core::db::embed_cache::Space;
use soul_core::db::Db;
use soul_core::derived::Derived;
use soul_core::obs::{ObsSet, Observation, Store};
use soul_core::paths::Paths;
use soul_core::rebuild::{self as core_rebuild, RebuildReport};
use soul_core::soulmd::NULL_GLYPH;

/// `soul render` — 파생값은 읽기만 하고 `SOUL.md`만 다시 쓴다.
pub fn render(paths: &Paths, offline: bool) -> Result<()> {
    let (derived, missing) = core_rebuild::replay(paths, offline)?;
    warn_misses(&missing);

    // §R8 — 커밋 메시지는 `render <T_ref ISO8601>`.
    // 관측이 하나도 없으면 T_ref가 없다. §R10대로 `—` 하나를 적는다.
    let t_ref = derived
        .t_ref
        .map(|t| t.to_rfc3339_millis())
        .unwrap_or_else(|| NULL_GLYPH.to_string());
    let report = core_rebuild::render_soul_md(paths, &derived, &format!("render {t_ref}"))?;
    report_render(paths, &report, &derived);
    Ok(())
}

/// `soul rebuild [--from-scratch] [--offline]` — 관측 재생으로 파생값을 갱신한다.
pub fn rebuild(paths: &Paths, from_scratch: bool, offline: bool) -> Result<()> {
    if from_scratch {
        clear_derived(paths)?;
    }

    // 1차 재생. `--offline`이면 캐시 미스에서 그대로 에러 종료한다 (T28).
    let (derived, missing) = core_rebuild::replay(paths, offline)?;

    let derived = if missing.is_empty() {
        derived
    } else {
        // §R3 — 재현성의 유일한 네트워크 예외. 채운 뒤 **다시 재생**한다.
        eprintln!(
            "임베딩 캐시 미스 {}건 — 새로 계산합니다 (§R3)",
            missing.len()
        );
        let filled = crate::cmd::block_on(backfill(paths, &missing))??;
        eprintln!("임베딩 {filled}건을 캐시에 넣었습니다.");
        let (derived, still_missing) = core_rebuild::replay(paths, false)?;
        if !still_missing.is_empty() {
            bail!(
                "임베딩을 채운 뒤에도 캐시 미스가 {}건 남았습니다",
                still_missing.len()
            );
        }
        derived
    };

    // §R8 — 커밋 메시지는 `rebuild <관측 수>`. 로그 전체 건수다.
    let n = derived.total_observation_count;
    let report = core_rebuild::render_soul_md(paths, &derived, &format!("rebuild {n}"))?;
    report_render(paths, &report, &derived);
    Ok(())
}

/// `--from-scratch` — 파생 테이블만 비운다.
///
/// **`derived.sqlite` 파일을 지우지 않는다.** 지우면 임베딩 캐시가 함께 사라져
/// T2("임베딩 캐시 테이블은 보존한 채 파생값 동일")가 성립하지 않는다.
fn clear_derived(paths: &Paths) -> Result<()> {
    let db_path = paths.derived_db();
    if !db_path.is_file() {
        // 없으면 비울 것도 없다. 읽기 경로에서 새 파일을 만들지 않는다.
        return Ok(());
    }
    let db = Db::open(&db_path).with_context(|| format!("{} 열기", db_path.display()))?;
    db.clear_derived()?;
    let cached = db.embed_count()?;
    eprintln!("파생 테이블을 비웠습니다 (임베딩 캐시 {cached}건은 보존, T2).");
    Ok(())
}

/// 캐시에 없는 텍스트를 임베딩해 채운다. **`soul rebuild`에서만 부른다.**
///
/// `ingest`와 `context`는 §12.7의 공간에 연결까지 하고(`get_and_link`),
/// `reading.prose`는 텍스트 캐시까지만 채운다(`get`) — 응답문 벡터가 `obs_vec`에
/// 들어가면 군집이 "무엇을 좋아하는가"에서 흘러나간다 (§18-3, T49).
async fn backfill(paths: &Paths, missing: &[String]) -> Result<usize> {
    use std::collections::HashSet;

    let wanted: HashSet<&str> = missing.iter().map(String::as_str).collect();
    let app = soul_pipeline::App::open_or_init(paths.clone())?;
    let client = app.openai()?;
    let embedder = soul_net::embed::Embedder {
        db: &app.db,
        client: Some(&client),
        provider: core_rebuild::EMBED_PROVIDER.to_string(),
        model: app.config.embed.model.clone(),
        dims: app.config.embed.dims,
        offline: false,
    };

    let set = app.store.load_set()?;
    let dead = set.superseded_ids();
    let mut done = 0usize;
    // ULID 순으로 훑는다 — 같은 텍스트가 여러 관측에 있어도 첫 호출 뒤에는 캐시 히트다.
    for o in set.as_slice() {
        match o {
            Observation::Ingest(i) if !dead.contains(&i.id) => {
                if wanted.contains(i.machine.prose.as_str()) {
                    embedder
                        .get_and_link(Space::Object, i.id.as_str(), &i.machine.prose)
                        .await?;
                    done += 1;
                }
            }
            Observation::Context(c) => {
                if wanted.contains(c.critique.as_str()) {
                    embedder
                        .get_and_link(Space::Critique, c.id.as_str(), &c.critique)
                        .await?;
                    done += 1;
                }
            }
            Observation::Reading(r) => {
                if let Some(p) = r.prose.as_deref() {
                    if wanted.contains(p) {
                        embedder.get(p).await?;
                        done += 1;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(done)
}

fn warn_misses(missing: &[String]) {
    if missing.is_empty() {
        return;
    }
    eprintln!(
        "경고: 임베딩 캐시 미스 {}건 — coherence·군집 같은 값이 {NULL_GLYPH}로 렌더됩니다. \
         `soul rebuild`로 채우세요 (§R3)",
        missing.len()
    );
}

fn report_render(paths: &Paths, report: &RebuildReport, derived: &Derived) {
    for w in &report.warnings {
        eprintln!("경고: {w}");
    }
    let t_ref = derived
        .t_ref
        .map(|t| t.to_rfc3339_millis())
        .unwrap_or_else(|| NULL_GLYPH.to_string());
    println!(
        "관측 {}건 (활성 ingest {}건) · 기준 {t_ref}",
        report.observations, derived.observation_count
    );
    println!(
        "{} — {}",
        paths.soul_md().display(),
        if report.soul_md_changed {
            "재작성"
        } else {
            "변경 없음"
        }
    );
    match &report.commit {
        Some(c) => println!("커밋 {}", &c[..c.len().min(12)]),
        None => println!("커밋 없음 (변경 없음)"),
    }
}

/// `Store`를 쓰는 쪽이 매번 같은 문장을 반복하지 않도록 모아 둔다.
///
/// 활성 ingest 임베딩 수집과 군집 수는 `soul_core::derived::{object_vectors, cluster_k}`
/// 로 옮겼다 — CLI 와 MCP 가 같은 규칙을 써야 한다.
pub(crate) fn load_set(paths: &Paths) -> Result<ObsSet> {
    Ok(Store::new(paths.clone()).load_set()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use soul_core::config::Config;
    use soul_core::db::embed_cache::cache_key;
    use soul_core::ids::ObsId;
    use soul_core::obs::{Axes, Ingest, Kind, Machine, ModelRef, Quality, Source};
    use soul_core::time::Ts;
    use soul_core::SCHEMA_VERSION;

    // ─────────────────────────────────────────────────────── 픽스처

    fn temp_paths() -> Paths {
        let root = std::env::temp_dir()
            .join("tasty-soul-cli-rebuild")
            .join(soul_core::ids::new_id().to_string());
        let p = Paths::at(root);
        p.ensure_dirs().unwrap();
        p
    }

    /// 오름차순이 보장되는 결정론적 ULID (§R2의 재생 순서).
    fn id(n: u32) -> ObsId {
        ObsId::parse(&format!("01J8XQZK3M7P4RSTVWXYZ{n:05}")).unwrap()
    }

    fn ts(day: u32) -> Ts {
        Ts::parse(&format!("2026-08-{day:02}T09:12:33.123Z")).unwrap()
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
            model: ModelRef {
                provider: "openai".into(),
                id: "gpt-x".into(),
                prompt_sha256: None,
                calls: vec![],
            },
            supersedes,
        })
    }

    fn write_obs(paths: &Paths, o: &Observation) {
        let path = paths.observation_file(o.ts(), o.id());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, o.to_canonical_json().unwrap()).unwrap();
    }

    /// `replay`가 쓰는 것과 **같은 캐시 키**로 임베딩을 채운다 (§R3).
    /// 여기서 키 계산이 어긋나면 워밍한 줄 알고 계속 미스가 난다.
    fn warm(paths: &Paths, texts: &[&str]) -> Db {
        let cfg = Config::load(&paths.config_toml()).unwrap();
        let db = Db::open(&paths.derived_db()).unwrap();
        for (i, t) in texts.iter().enumerate() {
            let key = cache_key(
                core_rebuild::EMBED_PROVIDER,
                &cfg.embed.model,
                cfg.embed.dims,
                t,
            );
            // 값 자체는 중요하지 않다 — 미스인지 히트인지만 본다. 결정론을 위해 고정한다.
            let v: Vec<f32> = (0..cfg.embed.dims)
                .map(|j| ((i + j) % 7) as f32 / 7.0)
                .collect();
            db.embed_put(&key, cfg.embed.dims, &v).unwrap();
        }
        db
    }

    fn latest_commit(paths: &Paths) -> String {
        soul_core::git::log_messages(&paths.soul(), 1)
            .unwrap()
            .into_iter()
            .next()
            .expect("커밋이 하나는 있어야 한다")
    }

    // ─────────────────────────────────────────────── T28 (--offline)

    /// T28 — `--offline`은 세 형태 **모두**에서 캐시 미스에 에러 종료한다.
    /// 그리고 에러로 끝났으면 `SOUL.md`를 쓰지 않는다 (§15 — 반쯤 렌더된 파일을 남기지 않는다).
    #[test]
    fn offline_errors_on_cache_miss_in_all_three_forms() {
        for (from_scratch, render_only) in [(false, true), (false, false), (true, false)] {
            let paths = temp_paths();
            write_obs(
                &paths,
                &ingest(1, "차갑고 정돈된, 사람이 방금 지워진 실내", None),
            );

            let r = if render_only {
                render(&paths, true)
            } else {
                rebuild(&paths, from_scratch, true)
            };
            assert!(
                r.is_err(),
                "from_scratch={from_scratch} render={render_only}"
            );
            assert!(
                !paths.soul_md().exists(),
                "에러로 끝났으면 SOUL.md를 남기지 않는다"
            );
        }
    }

    /// 캐시가 워밍되어 있으면 같은 명령이 네트워크 없이 성공한다 (§R3의 대우).
    #[test]
    fn offline_succeeds_when_the_cache_is_warm() {
        let paths = temp_paths();
        let prose = "차갑고 정돈된, 사람이 방금 지워진 실내";
        write_obs(&paths, &ingest(1, prose, None));
        drop(warm(&paths, &[prose]));

        rebuild(&paths, false, true).unwrap();
        assert!(paths.soul_md().is_file());
        render(&paths, true).unwrap();
    }

    // ─────────────────────────────────────────────── T2 (--from-scratch)

    /// T2 — `--from-scratch`는 파생 테이블만 비운다.
    ///
    /// `derived.sqlite` **파일을 지우지 않고**, `embed_cache`와 `obs_vec`을 보존한다.
    /// 파일을 지우면 다음 오프라인 재빌드가 전부 미스가 되어 T1·T2가 함께 깨진다.
    #[test]
    fn from_scratch_preserves_the_embed_cache_and_the_file() {
        let paths = temp_paths();
        let prose = "차갑고 정돈된, 사람이 방금 지워진 실내";
        write_obs(&paths, &ingest(1, prose, None));

        let db = warm(&paths, &[prose]);
        db.obs_vec_put(Space::Object, id(1).as_str(), &[0.5; 4])
            .unwrap();
        // 비워져야 하는 쪽도 하나 심어 둔다 — 아무것도 안 지우는 구현과 구별하기 위해서다.
        db.cluster_put(
            1,
            &soul_core::derived::cluster::Clustering {
                centroids: vec![vec![1.0, 0.0, 0.0, 0.0]],
                assignment: vec![0],
                k: 1,
            },
        )
        .unwrap();
        let before = db.embed_count().unwrap();
        assert_eq!(before, 1);
        drop(db);

        rebuild(&paths, true, true).unwrap();

        assert!(paths.derived_db().is_file(), "파일 자체를 지우면 안 된다");
        let db = Db::open(&paths.derived_db()).unwrap();
        assert_eq!(db.embed_count().unwrap(), before, "T2 — 임베딩 캐시 보존");
        assert!(
            db.obs_vec_get(Space::Object, id(1).as_str())
                .unwrap()
                .is_some(),
            "T2 — obs_vec 도 보존한다 (이게 남아야 오프라인으로 성립한다)"
        );
        assert!(db.cluster_get().unwrap().is_none(), "파생 캐시는 비운다");
    }

    /// 캐시 파일이 없을 때 `--from-scratch`가 파일을 **만들지 않는다.**
    /// 읽기 경로에서 빈 sqlite가 생기면 그다음 `--offline`이 미스로 죽는 대신 조용히 통과한다.
    #[test]
    fn from_scratch_does_not_create_a_missing_db() {
        let paths = temp_paths();
        clear_derived(&paths).unwrap();
        assert!(!paths.derived_db().exists());
    }

    // ─────────────────────────────────────────────── §R8 커밋 메시지

    /// §R8 — `render`는 `render <T_ref>`, `rebuild`는 `rebuild <관측 수>`로 커밋한다.
    /// 두 명령의 범위가 섞이면 여기가 먼저 티가 난다 (§14 표).
    #[test]
    fn commit_messages_follow_r8() {
        let paths = temp_paths();
        let a = "차갑고 정돈된, 사람이 방금 지워진 실내";
        write_obs(&paths, &ingest(1, a, None));
        drop(warm(&paths, &[a]));

        render(&paths, true).unwrap();
        assert_eq!(
            latest_commit(&paths),
            format!("render {}", ts(1).to_rfc3339_millis())
        );

        let b = "빛이 벽을 타고 내려오다 멈춘 자리";
        write_obs(&paths, &ingest(2, b, None));
        drop(warm(&paths, &[b]));
        rebuild(&paths, false, true).unwrap();
        assert_eq!(latest_commit(&paths), "rebuild 2", "로그 전체 건수다");
    }

    /// 관측이 하나도 없으면 T_ref가 없다. §R10대로 `—` 하나를 적는다.
    #[test]
    fn render_without_observations_uses_the_null_glyph() {
        let paths = temp_paths();
        render(&paths, true).unwrap();
        assert_eq!(latest_commit(&paths), format!("render {NULL_GLYPH}"));
    }
}
