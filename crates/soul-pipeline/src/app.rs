//! 앱 상태 — 경로·설정·DB·클라이언트를 한 곳에 묶는다.
//!
//! **상주 프로세스를 만들지 않는다** (§20.1). 워커는 이 구조체의 수명 안에서만 돌고,
//! 앱을 닫으면 함께 멈춘다 (T44).

use soul_core::config::Config;
use soul_core::db::Db;
use soul_core::error::Result;
use soul_core::obs::Store;
use soul_core::paths::Paths;

/// §3 최초 실행 3단계의 커밋 메시지.
///
/// §R8의 표에는 이 계기가 없다 — 표는 관측 추가·재렌더·재빌드 셋뿐이다.
/// 스켈레톤 커밋을 `render <T_ref>`로 적으면 관측이 0건이라 `T_ref`가 `null`이어서
/// `render —` 같은 메시지가 되고, `git log`가 성장 타임라인으로 읽히지 않는다.
/// 저장소 생성이라는 사실 그대로 적는다.
///
/// **`SOUL.md`를 명시한다** (§18-5 · T4c). 이 커밋은 `soul:human`이 빈 파일이 생긴
/// 지점이다. 사람이 쓴 글이 사라졌을 때 사용자는 `git log -p SOUL.md`를 거슬러 올라가는데,
/// 그때 "여기서부터 빈 파일로 새로 시작했다"가 로그에 보여야 어디까지 거슬러야 하는지
/// 알 수 있다. 그냥 `init`이면 저장소 초기화와 구별되지 않는다.
const INIT_COMMIT_MESSAGE: &str = "init SOUL.md";

pub struct App {
    pub paths: Paths,
    pub config: Config,
    pub db: Db,
    pub store: Store,
    pub tracer: Option<soul_core::trace::Tracer>,
    /// 이번 호출이 `SOUL.md`를 **새로 만들었는가** (§3 3단계 · §18-5).
    ///
    /// `open_or_init`이 스켈레톤을 자동 생성하기 때문에, 열고 난 뒤에 파일 유무를 보면
    /// 최초 실행인지 "사람이 쓴 `SOUL.md`가 사라진 뒤의 실행"인지 **구별할 수 없다.**
    /// 그 구별을 잃으면 `soul:human` 유실이 최초 실행처럼 조용히 지나간다 —
    /// `soul:human`은 관측 로그로 복원되지 않는 유일한 1차 자료다 (§R2 예외).
    ///
    /// 그래서 사실을 여기서 한 번만 붙잡아 호출자에게 넘긴다. 호출자(앱 셸·CLI)는
    /// 이 값으로 최초 실행 안내를 띄우거나, 파일이 사라졌음을 사용자에게 알린다.
    pub created_soul_md: bool,
}

impl App {
    /// 최초 실행 절차 (§3):
    /// 1. 루트 디렉토리 생성
    /// 2. `soul/`에 `git init`, `.gitattributes` 작성
    /// 3. `SOUL.md` 스켈레톤 생성 후 최초 커밋
    /// 4. `soul doctor` 자동 실행 → 키 미설정이면 설정 화면으로
    ///
    /// 4단계는 호출자(앱 셸·CLI)가 한다 — `doctor::run`은 async이고 네트워크를 쓸 수 있는데,
    /// 앱을 여는 것 자체는 오프라인에서 항상 성공해야 한다.
    pub fn open_or_init(paths: Paths) -> Result<App> {
        // 1. 루트와 하위 디렉토리 (§3).
        paths.ensure_dirs()?;

        // 2. `soul/` git 저장소. `.gitattributes`(`*.md text eol=lf`)와 `.gitignore`를
        //    함께 쓴다 (§R8·§D1 — remote는 만들지 않는다).
        soul_core::git::ensure_repo(&paths.soul())?;

        // 3. `SOUL.md` 스켈레톤 + 최초 커밋. 이미 있으면 건드리지 않는다 —
        //    사람이 쓴 `soul:human`을 덮어쓰는 일이 있어서는 안 된다 (§R2, T4).
        let soul_md = paths.soul_md();
        let created_soul_md = !soul_md.exists();
        if created_soul_md {
            // 원자적으로 쓴다. 여기서 중단되면 0바이트 `SOUL.md`가 남고, 그 파일은
            // "있지만 `soul:human` 블록이 없는" 상태라 이후 렌더가 이월할 것을 잃는다 (§18-5).
            soul_core::soulmd::save::write_soul_md_atomic(
                &soul_md,
                &soul_core::soulmd::render::skeleton(),
            )?;
            // `.gitattributes`가 이 커밋에 함께 들어가야 이후 체크아웃에서 CRLF로
            // 바뀌지 않는다 (T1). `.gitignore` 대상은 스테이징되지 않는다 (T20).
            soul_core::git::commit_all(&paths.soul(), INIT_COMMIT_MESSAGE)?;
        }

        // 설정. 파일이 없으면 기본값이며(§9.8), 사용자가 편집할 수 있도록 한 번 써 둔다.
        // `config.toml`은 `soul/` 밖이므로 커밋 대상이 아니다 (§3).
        let config = Config::load(&paths.config_toml())?;
        if !paths.config_toml().exists() {
            config.save(&paths.config_toml())?;
        }

        // 캐시 DB. `Db::open`이 이미 마이그레이션을 돌리지만 멱등이므로 명시적으로 한 번 더 부른다
        // — `cache/`는 삭제 가능한 디렉토리라(§3) 이 순서가 계약이라는 것을 코드로 남긴다.
        let db = Db::open(&paths.derived_db())?;
        db.migrate()?;
        // §9.10 T51c — 이전 실행이 큐 처리 중에 죽었으면 `running`이 남아 있다.
        // 앱 시작 시 `pending`으로 되돌려 이어받는다.
        db.queue_recover()?;

        let store = Store::new(paths.clone());
        // 트레이스는 재현성의 일부가 아니다 (§11.3). 열지 못해도 앱은 열려야 한다.
        let tracer = soul_core::trace::Tracer::open(&paths).ok();

        Ok(App {
            paths,
            config,
            db,
            store,
            tracer,
            created_soul_md,
        })
    }

    pub fn openai(&self) -> Result<soul_net::OpenAi> {
        // §2 — 키는 OS 키체인에만 있다. 이 값을 프런트로 돌려주는 경로를 만들지 않는다.
        let key = soul_net::secrets::get(soul_net::secrets::ACCOUNT_OPENAI)?
            .filter(|k| !k.trim().is_empty())
            // §15 — 키 미설정이면 모든 투입 경로가 비활성화된다. 사유를 분명히 말한다.
            .ok_or_else(|| {
                soul_core::error::SoulError::config(
                    "OpenAI API 키가 설정되지 않았습니다. 설정 화면에서 키를 입력하세요 (§9.9)",
                )
            })?;
        soul_net::OpenAi::new(&self.config, key)
    }

    /// §12의 파생값을 계산한다. 임베딩은 캐시에서만 읽는다.
    ///
    /// **`soul_core::rebuild::embedding_lookup` 을 쓴다.** 여기서 자체적으로 표를 만들면
    /// `soul render`/`soul rebuild`(= `rebuild::replay`)와 다른 `Derived`가 나온다 —
    /// 대시보드와 `SOUL.md`가 같은 로그를 두고 서로 다른 숫자를 말하게 되고,
    /// 그것이 §R2가 금지하는 어긋남이다. 아무것도 실패하지 않으므로 눈치채지 못한다.
    ///
    /// 조회를 텍스트 키로 하므로 `reading.prose` 벡터가 ID 색인(`obs_vec`)에 없어도 된다.
    /// 그것이 §12.7 불변식(ID 색인 = 투입 대상만)을 지탱한다.
    pub fn derived(&self) -> Result<soul_core::derived::Derived> {
        let set = self.store.load_set()?;
        let (embeds, _missing) =
            soul_core::rebuild::embedding_lookup(&set, Some(&self.db), &self.config)?;
        Ok(soul_core::derived::compute(
            &set,
            &embeds,
            self.config.local.silhouette_max_samples,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> std::path::PathBuf {
        std::env::temp_dir()
            .join("tasty-soul-app-test")
            .join(soul_core::ids::new_id().to_string())
    }

    /// §3 최초 실행 1~3단계.
    #[test]
    fn first_run_creates_repo_skeleton_and_commit() {
        let root = temp_root();
        let app = App::open_or_init(Paths::at(&root)).unwrap();

        assert!(app.paths.soul().is_dir());
        assert!(app.paths.soul().join(".gitattributes").is_file());
        assert!(app.paths.observations().is_dir());
        assert!(app.paths.runs().is_dir());
        assert!(app.paths.derived_db().is_file());
        assert!(app.paths.config_toml().is_file(), "기본 설정을 써 둔다");

        let md = std::fs::read_to_string(app.paths.soul_md()).unwrap();
        assert_eq!(md, soul_core::soulmd::render::skeleton());
        assert!(!md.contains('\r'), "SOUL.md는 LF다");
        // 전 블록이 비어 있는 스켈레톤이므로 파생값 자리는 전부 `—`다 (§R10).
        assert!(md.contains("관측 0"), "{md}");

        // `ensure_repo`가 `.gitattributes`/`.gitignore`를 먼저 커밋하고("init repo"),
        // 그 위에 스켈레톤 커밋이 얹힌다. 메타데이터를 커밋하지 않으면 새 체크아웃에서
        // `*.md text eol=lf`가 적용되지 않아 T1이 깨진다 (git.rs 참조).
        let log = soul_core::git::log_messages(&app.paths.soul(), 10).unwrap();
        assert_eq!(
            log,
            vec![INIT_COMMIT_MESSAGE.to_string(), "init repo".to_string()]
        );
        assert!(
            soul_core::git::is_clean(&app.paths.soul()).unwrap(),
            "T20 — .write.lock과 SOUL.next.md는 git status에 나타나지 않는다"
        );
    }

    /// §18-5 — 스켈레톤을 **새로 만들었다는 사실**이 호출자에게도 `git log`에도 남는다.
    ///
    /// `open_or_init`이 자동 생성하기 때문에 열고 나서 파일 유무를 보면 최초 실행과
    /// "`SOUL.md`가 사라진 뒤의 실행"이 구별되지 않는다. 그 구별이 사라지면
    /// `soul:human` 유실이 최초 실행처럼 조용히 지나간다.
    #[test]
    fn open_reports_skeleton_creation_and_marks_it_in_the_log() {
        let root = temp_root();

        let first = App::open_or_init(Paths::at(&root)).unwrap();
        assert!(first.created_soul_md, "이번 실행이 SOUL.md를 만들었다");
        let log = soul_core::git::log_messages(&first.paths.soul(), 10).unwrap();
        assert!(
            log[0].contains("SOUL.md"),
            "빈 soul:human이 시작된 지점을 `git log`에서 찾을 수 있어야 한다: {log:?}"
        );
        drop(first);

        let second = App::open_or_init(Paths::at(&root)).unwrap();
        assert!(
            !second.created_soul_md,
            "이미 있으면 만들지 않았다고 말해야 한다"
        );
        drop(second);

        // 사람이 쓴 파일이 사라진 뒤의 실행 — 겉보기에는 최초 실행과 똑같다.
        std::fs::remove_file(Paths::at(&root).soul_md()).unwrap();
        let third = App::open_or_init(Paths::at(&root)).unwrap();
        assert!(
            third.created_soul_md,
            "사라진 SOUL.md를 다시 만든 것도 호출자가 알아야 한다"
        );
    }

    /// 두 번째 실행은 아무것도 덮어쓰지 않는다.
    #[test]
    fn reopen_is_idempotent_and_preserves_soul_md() {
        let root = temp_root();
        {
            let app = App::open_or_init(Paths::at(&root)).unwrap();
            // 사람이 쓴 내용이 들어간 상태를 흉내 낸다 (§R2, T4).
            let edited = std::fs::read_to_string(app.paths.soul_md())
                .unwrap()
                .replace("<!-- soul:human -->\n", "<!-- soul:human -->\n내가 쓴 줄\n");
            std::fs::write(app.paths.soul_md(), &edited).unwrap();
        }
        let app = App::open_or_init(Paths::at(&root)).unwrap();
        let md = std::fs::read_to_string(app.paths.soul_md()).unwrap();
        assert!(md.contains("내가 쓴 줄"), "두 번째 열기가 덮어쓰면 안 된다");
        // 커밋이 늘지 않는다 ("init repo" + "init SOUL.md" 그대로).
        assert_eq!(
            soul_core::git::log_messages(&app.paths.soul(), 10)
                .unwrap()
                .len(),
            2
        );
    }

    /// 관측이 하나도 없으면 §R1의 `T_ref`가 없고 파생값도 전부 비어 있다.
    #[test]
    fn derived_on_empty_store_is_all_null() {
        let root = temp_root();
        let app = App::open_or_init(Paths::at(&root)).unwrap();
        let d = app.derived().unwrap();
        assert_eq!(d.t_ref, None);
        assert_eq!(d.observation_count, 0);
        assert_eq!(d.axes_final, None);
        assert_eq!(d.coherence_sensory, None);
        assert_eq!(d.coherence_cultural, None);
    }

    // ────────────────────────── §12.7 두 공간 (T49) — `derived()`의 맵 합치기

    use soul_core::db::embed_cache::Space;

    use soul_core::obs::{
        Axes, ContextObs, Ingest, Kind, Layer, Machine, ModelRef, Observation, Quality, Reading,
        Source, SourceRef, Verdict,
    };
    use std::collections::BTreeMap;

    fn model_ref() -> ModelRef {
        ModelRef {
            provider: "openai".into(),
            id: "m".into(),
            prompt_sha256: None,
            calls: vec![],
        }
    }

    fn push_ingest(app: &App, prose: &str, vec: &[f32]) -> soul_core::ids::ObsId {
        let (id, ts, schema) = soul_core::obs::new_header();
        let o = Observation::Ingest(Ingest {
            id: id.clone(),
            ts,
            schema,
            source: Source {
                kind: Kind::Image,
                sha256: format!("{prose:x>8}"),
                origin: format!("file:///tmp/{id}.jpg"),
                bytes: 1,
                mime: "image/jpeg".into(),
            },
            machine: Machine {
                prose: prose.into(),
                axes: Axes::ZERO,
                tags: vec![],
                quality: Quality::Full,
                prompt_sha256: "sha".into(),
            },
            min_dist: None,
            surprisal: 0.0,
            model: model_ref(),
            supersedes: None,
        });
        app.store.append(&o).unwrap();
        app.db.obs_vec_put(Space::Object, id.as_str(), vec).unwrap();
        id
    }

    fn push_context(
        app: &App,
        target: &soul_core::ids::ObsId,
        critique: &str,
    ) -> soul_core::ids::ObsId {
        let (id, ts, schema) = soul_core::obs::new_header();
        let src = |u: &str| SourceRef {
            url: u.into(),
            title: u.into(),
            fetched_at: ts,
        };
        let o = Observation::Context(ContextObs {
            id: id.clone(),
            ts,
            schema,
            target: target.clone(),
            critique: critique.into(),
            lineage: vec![],
            queries: vec!["q".into()],
            sources: vec![src("https://a.example"), src("https://b.example")],
            grounded: true,
            model: model_ref(),
        });
        app.store.append(&o).unwrap();
        id
    }

    fn push_cultural_reading(
        app: &App,
        ctx: &soul_core::ids::ObsId,
        prose: &str,
    ) -> soul_core::ids::ObsId {
        let (id, ts, schema) = soul_core::obs::new_header();
        let o = Observation::Reading(Reading {
            id: id.clone(),
            ts,
            schema,
            layer: Layer::Cultural,
            target: ctx.clone(),
            verdict: Verdict::No,
            prose: Some(prose.into()),
            divergence: Some(0.4),
        });
        app.store.append(&o).unwrap();
        id
    }

    /// 텍스트 키 임베딩 캐시를 워밍한다 (§R3의 캐시 키).
    ///
    /// 파생값 계산 경로는 전부 이 캐시를 텍스트로 조회한다. 테스트가 `obs_vec` 에
    /// 직접 써 넣으면 실제 경로를 재현하지 못한다.
    fn warm(app: &App, text: &str, dir: &[f32]) {
        let dims = app.config.embed.dims;
        // 캐시는 키의 dims 와 벡터 길이가 일치해야 받는다. 방향만 주고 나머지는 0으로 채운다.
        let mut v = vec![0.0f32; dims];
        v[..dir.len()].copy_from_slice(dir);
        let key = soul_core::db::embed_cache::cache_key(
            soul_core::rebuild::EMBED_PROVIDER,
            &app.config.embed.model,
            dims,
            text,
        );
        app.db.embed_put(&key, dims, &v).unwrap();
    }

    /// T49 · §12.7 — Critique 공간을 같은 맵에 합쳐도 **군집 입력은 오염되지 않는다.**
    ///
    /// 두 주장을 한 번에 세운다.
    /// 1. 합친 맵의 결과가 Object 공간만 넘긴 기준 계산과 **군집 관련 필드에서 동일**하다.
    /// 2. 그런데도 `coherence_cultural`은 합쳐야만 나온다 — 합치는 이유가 실재한다.
    #[test]
    fn derived_matches_replay_and_keeps_clustering_clean() {
        let root = temp_root();
        let app = App::open_or_init(Paths::at(&root)).unwrap();

        // 활성 ingest 3건. 이들의 `machine.prose`만이 군집 입력이다.
        let i0 = push_ingest(&app, "차갑고 정돈된 실내", &[1.0, 0.0, 0.0, 0.0]);
        let i1 = push_ingest(&app, "젖은 아스팔트의 밤", &[0.0, 1.0, 0.0, 0.0]);
        push_ingest(&app, "빛이 바랜 여름 마당", &[0.0, 0.0, 1.0, 0.0]);
        warm(&app, "차갑고 정돈된 실내", &[1.0, 0.0, 0.0, 0.0]);
        warm(&app, "젖은 아스팔트의 밤", &[0.0, 1.0, 0.0, 0.0]);
        warm(&app, "빛이 바랜 여름 마당", &[0.0, 0.0, 1.0, 0.0]);

        // 문화 층 2단 조인: ingest → context → cultural reading (§12.6).
        let c0 = push_context(&app, &i0, "미니멀리즘 실내 사진의 계보 위에 있다");
        let c1 = push_context(&app, &i1, "느와르 도시 이미지의 관습을 따른다");
        push_cultural_reading(&app, &c0, "계보보다 그냥 텅 빈 느낌이 먼저다");
        push_cultural_reading(&app, &c1, "느와르라기보다 퇴근길에 가깝다");
        // 비평 공간의 방향을 대상 공간과 겹치지 않게 둔다 —
        // 섞였다면 군집이 반드시 흔들린다.
        warm(
            &app,
            "미니멀리즘 실내 사진의 계보 위에 있다",
            &[0.0, 0.0, 0.0, 1.0],
        );
        warm(
            &app,
            "느와르 도시 이미지의 관습을 따른다",
            &[0.0, 0.0, 0.0, -1.0],
        );
        warm(
            &app,
            "계보보다 그냥 텅 빈 느낌이 먼저다",
            &[0.0, 0.0, 1.0, 1.0],
        );
        warm(
            &app,
            "느와르라기보다 퇴근길에 가깝다",
            &[0.0, 0.0, 1.0, -1.0],
        );

        let actual = app.derived().unwrap();

        // ── 주장 1 (§R2). 대시보드가 쓰는 경로와 `soul render`/`soul rebuild` 가 쓰는
        //    경로가 **같은 로그에서 같은 값**을 내야 한다. 갈라지면 화면과 문서가
        //    서로 다른 숫자를 말하고, 아무것도 실패하지 않아 눈치채지 못한다.
        let (replayed, missing) = soul_core::rebuild::replay(&app.paths, true).unwrap();
        assert!(
            missing.is_empty(),
            "캐시를 워밍했으므로 미스가 없어야 한다: {missing:?}"
        );
        assert_eq!(actual, replayed, "§R2 — 두 파생값 경로가 어긋나면 안 된다");

        // ── 주장 2 (T49). 군집 입력은 활성 ingest 3건뿐이다.
        //    비평 벡터가 섞였다면 4차원 축이 흔들려 이 기준과 달라진다.
        let set = app.store.load_set().unwrap();
        let mut object_only: BTreeMap<String, Vec<f32>> = BTreeMap::new();
        for i in set.active_ingests() {
            let v = app
                .db
                .embed_get(&soul_core::db::embed_cache::cache_key(
                    soul_core::rebuild::EMBED_PROVIDER,
                    &app.config.embed.model,
                    app.config.embed.dims,
                    &i.machine.prose,
                ))
                .unwrap()
                .unwrap();
            object_only.insert(i.machine.prose.clone(), v);
        }
        assert_eq!(object_only.len(), 3, "군집 입력은 ingest 3건뿐이다");
        let reference = soul_core::derived::compute(
            &set,
            &object_only,
            app.config.local.silhouette_max_samples,
        );
        assert_eq!(actual.axes_final, reference.axes_final);
        assert_eq!(
            actual.timeline, reference.timeline,
            "T49 — 군집이 흔들리면 안 된다"
        );
        assert_eq!(actual.crystal_now, reference.crystal_now);
        assert_eq!(actual.coherence_sensory, reference.coherence_sensory);
        assert_eq!(actual.observation_count, 3);

        // ── 주장 3. 그런데도 문화 층 coherence 는 실제로 계산된다 —
        //    비평 공간을 조회하지 못하면 영원히 null 이므로, 주장 2가 공허하지 않다.
        assert!(reference.coherence_cultural.is_none());
        assert!(
            actual.coherence_cultural.is_some(),
            "텍스트 키 조회가 비평 공간까지 덮어야 문화 층이 산다"
        );
    }

    /// §9.10 T51c — 열 때 `running`이 `pending`으로 되돌아온다.
    #[test]
    fn open_recovers_running_queue_items() {
        let root = temp_root();
        let id = soul_core::ids::new_id().to_string();
        {
            let app = App::open_or_init(Paths::at(&root)).unwrap();
            app.db.queue_push(&id).unwrap();
            let claimed = app.db.queue_claim().unwrap().expect("하나를 집는다");
            assert_eq!(claimed.ingest_id, id);
            assert_eq!(app.db.queue_pending_count().unwrap(), 0);
        }
        let app = App::open_or_init(Paths::at(&root)).unwrap();
        assert_eq!(
            app.db.queue_pending_count().unwrap(),
            1,
            "중간에 죽은 작업을 이어받아야 한다"
        );
    }
}
