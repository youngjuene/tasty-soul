//! 편집 저장 시퀀스 (§8.4).
//!
//! ## 순서를 정확히 이 순서로 수행한다. 바뀌면 편집이 유실되거나 해시가 어긋난다.
//!
//! 1. 쓰기 락 획득 (§R7). 실패하면 **즉시 중단하고 아무것도 쓰지 않는다.**
//! 2. 편집된 내용을 파싱한다. 마커 짝이 안 맞으면 **여기서 중단**하고 파일을 되돌리지 않는다
//!    (사용자 편집 상태 유지).
//! 3. `soul:human` 본문을 **메모리에 그대로 보관**한다.
//! 4. 각 `soul:neg` 블록의 해시를 비교해 변경된 것마다 `profile_edit` 관측을 기록하고 **커밋한다.**
//! 5. 재렌더한다 — `soul:gen`은 현재 파생값으로, `soul:neg`는 방금 기록한 `to_text`로,
//!    `soul:human`은 **3에서 보관한 내용 그대로** 채운다.
//! 6. `rev` +1, `hash`를 새 값으로 갱신한다.
//! 7. 파일을 쓰고 `render <T_ref>` 커밋.
//! 8. 락 해제.
//!
//! **저장 한 번에 커밋이 여러 개 생긴다** (변경된 `soul:neg` 블록 수 + 재렌더 1). 정상이다 (T21c).
//!
//! 사용자가 `soul:human`만 편집한 경우 4단계에서 관측이 하나도 안 생기지만,
//! 파일 내용은 바뀌었으므로 **5~7은 그대로 수행한다.** 이 경우 커밋은 `render <T_ref>` 하나뿐이다 (T4).

use crate::config::Config;
use crate::db::embed_cache::Space;
use crate::db::Db;
use crate::derived;
use crate::error::{Result, SoulError};
use crate::git;
use crate::ids::ObsId;
use crate::lock::WriteLock;
use crate::obs::model::{new_header, Observation, ProfileEdit};
use crate::obs::{ObsSet, Store};
use crate::paths::Paths;
use crate::soulmd::{parse, render, BlockKind, RenderInput};
use crate::time::Ts;
use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::Path;

/// §8.2 템플릿의 유일한 `soul:neg` 블록 id. 렌더러가 받는 `profile_text`가 이 블록이다.
const PROFILE_BLOCK_ID: &str = "profile";

/// §6.7 `profile_edit.author`. 이 경로는 사람의 편집만 만든다.
const AUTHOR_HUMAN: &str = "human";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SaveOutcome {
    /// 기록된 `profile_edit` 관측 ID들 (§8.4 4단계).
    pub profile_edits: Vec<ObsId>,
    /// 생성된 커밋 수 = `profile_edits.len() + 1`.
    pub commits: usize,
    /// §8.3 규칙 7 — `soul:gen`이 편집되었음. 사용자에게 알린다.
    pub gen_blocks_modified: Vec<String>,
}

/// 원자적 쓰기의 임시 파일 이름. **같은 디렉토리**에 둔다 (아래 함수 주석 참고).
///
/// 이름을 고정하는 이유: 중간에 죽어 남은 찌꺼기를 다음 쓰기가 그냥 덮어쓴다.
/// 매번 다른 이름을 쓰면 실패할 때마다 `soul/`에 쓰레기가 하나씩 쌓인다.
/// 점으로 시작하므로 파일 목록에서 사용자 눈에 걸리지 않는다.
const SOUL_MD_TMP: &str = ".SOUL.md.tmp";

/// `SOUL.md`를 **원자적으로** 쓴다 — 같은 디렉토리의 임시 파일에 다 쓰고 `rename`한다.
///
/// ## 왜 `fs::write`가 아닌가 (§18-5 · T4b)
///
/// `fs::write`는 대상 파일을 **먼저 0바이트로 자르고** 나서 내용을 쓴다. 그 사이에
/// 프로세스가 죽거나(사용자가 앱을 강제 종료한다) 디스크가 차면 **0바이트 `SOUL.md`**가
/// 남는다. `soul:human`은 어떤 관측도 만들지 않는 유일한 1차 자료라(§R2 예외) 관측
/// 로그로 복원할 수 없다 — 그 창에서 죽으면 사용자가 직접 쓴 문장이 통째로 사라지고,
/// 아무것도 실패하지 않았으므로 다음 렌더는 조용히 빈 블록을 써 낸다.
///
/// POSIX의 `rename(2)`은 같은 파일 시스템 안에서 원자적이다. 임시 파일을 **같은
/// 디렉토리**에 만드는 이유가 이것이다. `std::env::temp_dir()`에 만들면 파일 시스템이
/// 달라져 rename이 복사+삭제로 떨어지고 원자성이 사라진다.
///
/// **이 함수를 `fs::write` 한 줄로 되돌리지 말 것.** 되돌려도 테스트는 대부분 통과한다.
/// 깨지는 것은 "쓰다가 죽은 다음 실행"뿐이고, 그때는 이미 복구할 원본이 없다.
pub fn write_soul_md_atomic(path: &Path, text: &str) -> Result<()> {
    let dir = path.parent().ok_or_else(|| {
        SoulError::invalid(format!(
            "SOUL.md 경로에 부모 디렉토리가 없습니다: {}",
            path.display()
        ))
    })?;
    let tmp = dir.join(SOUL_MD_TMP);

    let written = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        // rename이 원자적이어도 내용이 아직 디스크에 닿지 않았으면 전원이 나간 뒤
        // **0바이트로 보이는** 파일이 남을 수 있다. 그것이 막으려는 상태 그 자체다.
        f.sync_all()
    })();
    if let Err(e) = written {
        // 반쯤 쓰인 임시 파일을 남기지 않는다 — 다음 실행이 그것을 보고 헷갈릴 이유가 없다.
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }

    std::fs::rename(&tmp, path)?;

    // 디렉토리 엔트리까지 내려야 rename 자체가 정전을 넘긴다. 지원하지 않는 파일
    // 시스템도 있으므로 실패는 삼킨다 — 여기까지 왔으면 파일 내용은 이미 온전하다.
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// 편집된 전문을 받아 §8.4 순서대로 저장한다.
///
/// `edited`는 사용자가 편집 모드에서 저장한 문서 전문이다.
pub fn save_edited(paths: &Paths, edited: &str) -> Result<SaveOutcome> {
    let soul_dir = paths.soul();

    // ── 1. 쓰기 락. 실패하면 여기서 끝이다. 아무것도 쓰지 않는다 (§R7).
    let lock = WriteLock::acquire(&soul_dir)?;

    // ── 2. 파싱. 마커 짝이 안 맞으면 `?`로 여기서 중단한다.
    //       디스크의 `SOUL.md`는 손대지 않았으므로 사용자 편집 상태가 그대로 남는다 (T10).
    let doc = parse(edited)?;

    // ── 3. `soul:human` 본문을 메모리에 그대로 보관한다.
    //       이 값이 5단계까지 살아 있어야 자유 기록이 조용히 사라지지 않는다 (§18-5, T4).
    let human_text = doc.human_body().unwrap_or("").to_string();

    // §8.3 규칙 7 — 직전 파일과 비교해 `soul:gen` 편집을 찾아낸다. 경고만 하고 그대로 진행한다 (T5).
    let previous = std::fs::read_to_string(paths.soul_md())
        .ok()
        .and_then(|s| parse(&s).ok());
    let mut gen_blocks_modified = Vec::new();
    if let Some(prev) = previous.as_ref() {
        for b in doc.blocks.iter().filter(|b| b.kind == BlockKind::Gen) {
            let Some(id) = b.id.as_deref() else { continue };
            if let Some(old) = prev.block(id) {
                if old.actual_hash() != b.actual_hash() {
                    gen_blocks_modified.push(id.to_string());
                }
            }
        }
    }

    // ── 4. 변경된 `soul:neg` 블록마다 `profile_edit` 관측을 기록하고 **각각 커밋한다.**
    //       쓰기 1회 = 커밋 1개다. 배치 커밋을 하지 않는다 (§R8).
    let store = Store::new(paths.clone());
    let mut profile_edits: Vec<ObsId> = Vec::new();
    let mut commits = 0usize;
    for b in doc.dirty_neg_blocks() {
        let Some(block_id) = b.id.as_deref() else {
            continue;
        };
        let (id, ts, schema) = new_header();
        let obs = Observation::ProfileEdit(ProfileEdit {
            id: id.clone(),
            ts,
            schema,
            block: block_id.to_string(),
            // 마커가 주장하던 옛 해시. 재생 시 어느 텍스트를 갈아끼우는지가 이것으로 정해진다.
            from_hash: b.hash.clone().unwrap_or_default(),
            to_text: b.normalized_body(),
            author: AUTHOR_HUMAN.to_string(),
        });
        obs.validate()?;
        let written = store.append(&obs)?;
        let rel = written.strip_prefix(&soul_dir).map_err(|_| {
            SoulError::invalid(format!(
                "관측 파일이 soul/ 밖에 쓰였습니다: {}",
                written.display()
            ))
        })?;
        if git::commit_paths(&soul_dir, &[rel], &format!("{} {}", obs.type_name(), id))?.is_some() {
            commits += 1;
        }
        profile_edits.push(id);
    }

    // ── 5. 재렌더. 파생값은 방금 기록한 관측까지 포함한 상태에서 계산한다.
    store.invalidate();
    let set = store.load_set()?;
    let cfg = Config::load(&paths.config_toml())?;
    let db = open_derived_db(paths)?;
    let embeds = object_space_embeddings(&db, &set)?;
    let derived = derived::compute(&set, &embeds, cfg.local.silhouette_max_samples);

    let profile = doc.block(PROFILE_BLOCK_ID);
    // `soul:neg`는 방금 관측에 기록한 `to_text`와 같은 값이다 (편집되지 않았으면 원래 본문).
    let profile_text = profile.map(|b| b.normalized_body()).unwrap_or_default();
    // ── 6. rev +1. 편집된 블록만 올린다 (§8.3 규칙 4). `hash`는 렌더러가 새 본문으로 다시 박는다.
    let profile_rev = profile
        .map(|b| {
            let cur = b.rev.unwrap_or(0);
            if b.is_dirty() {
                cur.saturating_add(1)
            } else {
                cur
            }
        })
        .unwrap_or(0);
    // §8.2 어긋남 블록의 대표 사례는 sensory 쪽을 쓴다 (docs/OPEN-DECISIONS.md #17).
    let examples: Vec<ObsId> = derived
        .coherence_sensory
        .as_ref()
        .map(|c| c.examples.clone())
        .unwrap_or_default();

    let rendered = render(&RenderInput {
        derived: &derived,
        profile_text: &profile_text,
        profile_rev,
        human_text: &human_text,
        divergence_examples: &examples,
    });

    // ── 7. 파일을 쓰고 `render <T_ref>` 커밋.
    //       원자적으로 쓴다 — 여기서 죽으면 방금 3단계에 보관한 `soul:human`이
    //       0바이트 파일과 함께 사라진다 (§18-5).
    write_soul_md_atomic(&paths.soul_md(), &rendered)?;
    let t_ref = derived
        .t_ref
        // 관측이 하나도 없는 문서에는 T_ref가 없다. 그때만 벽시계를 쓴다 (§R1이 허용하는
        // 용처는 아니지만, 커밋 메시지는 파생값이 아니다 — §R8 주석과 같은 취지다).
        .unwrap_or_else(Ts::now)
        .to_rfc3339_millis();
    if git::commit_paths(
        &soul_dir,
        &[Path::new("SOUL.md")],
        &format!("render {t_ref}"),
    )?
    .is_some()
    {
        commits += 1;
    }

    // ── 8. 락 해제.
    drop(lock);

    Ok(SaveOutcome {
        profile_edits,
        commits,
        gen_blocks_modified,
    })
}

/// `cache/derived.sqlite`를 연다. 캐시 디렉토리는 지워질 수 있으므로(§3) 없으면 만든다.
fn open_derived_db(paths: &Paths) -> Result<Db> {
    let path = paths.derived_db();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Db::open(&path)
}

/// §12.7 — **`machine.prose` 공간(`obs_vec`)만** 조회기로 넘긴다.
///
/// `critique_vec`을 섞으면 군집이 "무엇을 좋아하는가"에서 "무엇에 관해 글이 쓰였는가"로
/// 흐른다 (§18-3, T49). 그래서 `Space::Object` 하나만 읽는다.
fn object_space_embeddings(db: &Db, set: &ObsSet) -> Result<BTreeMap<String, Vec<f32>>> {
    let stored: HashMap<String, Vec<f32>> = db.obs_vec_all(Space::Object)?.into_iter().collect();
    let mut out = BTreeMap::new();
    for i in set.active_ingests() {
        if let Some(v) = stored.get(i.id.as_str()) {
            out.insert(i.machine.prose.clone(), v.clone());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obs::model::{Axes, Ingest, Kind, Machine, ModelRef, Quality, Source};
    use crate::soulmd::block_hash;
    use crate::SCHEMA_VERSION;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    // ────────────────────────────────────────────────────────── 임시 루트

    static SEQ: AtomicU32 = AtomicU32::new(0);

    struct TempRoot(PathBuf);

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn setup(tag: &str) -> (TempRoot, Paths) {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "tasty-soul-save-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&p);
        let paths = Paths::at(p.clone());
        paths.ensure_dirs().expect("디렉토리 생성");
        git::ensure_repo(&paths.soul()).expect("git init");
        (TempRoot(p), paths)
    }

    // ────────────────────────────────────────────────────────── 픽스처

    fn gen_block(id: &str, body: &str) -> String {
        format!("<!-- soul:gen id={id} -->\n{body}\n<!-- /soul:gen -->\n")
    }

    /// `hash_src`로 마커 해시를 계산하고 본문은 `body`로 쓴다.
    /// 둘을 다르게 주면 "편집된 블록"이 된다.
    fn neg_block(id: &str, rev: u32, hash_src: &str, body: &str) -> String {
        format!(
            "<!-- soul:neg id={id} rev={rev} hash={} -->\n{body}\n<!-- /soul:neg -->\n",
            block_hash(hash_src)
        )
    }

    fn human_block(body: &str) -> String {
        format!("<!-- soul:human -->\n{body}\n<!-- /soul:human -->\n")
    }

    const HEADER_BODY: &str = "기준 2026-08-13 · 관측 1 · 최초 2026-08-13\n어긋남 — · 해상도 —";

    /// §8.2 템플릿 모양의 문서. `soul:neg`는 인자로 받은 것들을 그대로 싣는다.
    fn doc_text(header: &str, negs: &[String], human: &str) -> String {
        let mut s = String::from("# SOUL\n\n");
        s.push_str(&gen_block("header", header));
        s.push_str("\n## 지금의 취향\n\n");
        for n in negs {
            s.push_str(n);
            s.push('\n');
        }
        s.push_str("## 내가 쓴 것\n\n");
        s.push_str(&human_block(human));
        s
    }

    /// `soul:neg` 하나짜리 표준 문서.
    fn simple_doc(profile_hash_src: &str, profile_body: &str, rev: u32, human: &str) -> String {
        doc_text(
            HEADER_BODY,
            &[neg_block(
                PROFILE_BLOCK_ID,
                rev,
                profile_hash_src,
                profile_body,
            )],
            human,
        )
    }

    fn seed_ingest(paths: &Paths) {
        let store = Store::new(paths.clone());
        let (id, ts, schema) = new_header();
        let obs = Observation::Ingest(Ingest {
            id: id.clone(),
            ts,
            schema,
            source: Source {
                kind: Kind::Image,
                sha256: "abcd1234".into(),
                origin: "file:///tmp/a.jpg".into(),
                bytes: 1024,
                mime: "image/jpeg".into(),
            },
            machine: Machine {
                prose: "차갑고 정돈된, 사람이 방금 지워진 실내".into(),
                axes: Axes::from_array([0.2, 0.4, 0.3, 0.6, 0.1, 0.7, 0.3, 0.2]),
                tags: vec!["실내".into()],
                quality: Quality::Full,
                prompt_sha256: "9f2c1a".into(),
            },
            min_dist: None,
            surprisal: 1.0,
            model: ModelRef {
                provider: "openai".into(),
                id: "gpt-x".into(),
                prompt_sha256: None,
                calls: vec![],
            },
            supersedes: None,
        });
        assert_eq!(schema, SCHEMA_VERSION);
        let written = store.append(&obs).expect("관측 기록");
        let rel = written.strip_prefix(paths.soul()).unwrap();
        git::commit_paths(&paths.soul(), &[rel], &format!("ingest {id}")).expect("커밋");
    }

    fn write_soul_md(paths: &Paths, text: &str) {
        std::fs::write(paths.soul_md(), text).expect("SOUL.md 쓰기");
        git::commit_paths(&paths.soul(), &[Path::new("SOUL.md")], "render fixture").expect("커밋");
    }

    fn commit_count(paths: &Paths) -> usize {
        git::log_messages(&paths.soul(), 10_000)
            .expect("git log")
            .len()
    }

    fn obs_count(paths: &Paths) -> usize {
        Store::new(paths.clone()).count().expect("관측 수")
    }

    fn read_soul_md(paths: &Paths) -> String {
        std::fs::read_to_string(paths.soul_md()).expect("SOUL.md 읽기")
    }

    // ────────────────────────────────────────────────────────── 테스트

    /// T4 — `soul:human`만 편집 → 관측 0건, `render` 커밋 1개, 내용 보존.
    #[test]
    fn t4_human_only_edit_makes_no_observation_and_one_commit() {
        let (_tmp, paths) = setup("t4");
        seed_ingest(&paths);
        let profile = "인공물보다 방치된 것을 고른다.";
        write_soul_md(&paths, &simple_doc(profile, profile, 17, "예전에 쓴 문장"));
        let commits_before = commit_count(&paths);
        let obs_before = obs_count(&paths);

        // human 본문만 바꾼다. soul:neg의 본문과 마커 해시는 그대로다.
        let edited = simple_doc(
            profile,
            profile,
            17,
            "새로 쓴 문장. 습도라는 말이 자꾸 걸린다.",
        );
        let out = save_edited(&paths, &edited).expect("저장");

        assert!(
            out.profile_edits.is_empty(),
            "human만 고쳤으면 관측이 하나도 생기지 않는다"
        );
        assert_eq!(out.commits, 1, "render 커밋 하나뿐이다");
        assert_eq!(
            obs_count(&paths),
            obs_before,
            "관측 파일이 늘지 않아야 한다"
        );
        assert_eq!(commit_count(&paths), commits_before + 1);

        let saved = read_soul_md(&paths);
        assert!(
            saved.contains("새로 쓴 문장. 습도라는 말이 자꾸 걸린다."),
            "3단계에서 보관한 human 본문이 그대로 실려야 한다:\n{saved}"
        );
        let last = git::log_messages(&paths.soul(), 1).expect("git log");
        assert!(
            last[0].starts_with("render "),
            "마지막 커밋 메시지가 `render <T_ref>` 여야 한다: {last:?}"
        );
    }

    /// T21c — `soul:neg` 2개 동시 편집 → `profile_edit` 2건 + `render` 1건 = 커밋 3개.
    #[test]
    fn t21c_two_edited_neg_blocks_produce_three_commits() {
        let (_tmp, paths) = setup("t21c");
        seed_ingest(&paths);
        let a0 = "인공물보다 방치된 것을 고른다.";
        let b0 = "채도가 아니라 습도로 공간을 읽는다.";
        let original = doc_text(
            HEADER_BODY,
            &[
                neg_block(PROFILE_BLOCK_ID, 17, a0, a0),
                neg_block("stance", 4, b0, b0),
            ],
            "사람이 쓴 문장",
        );
        write_soul_md(&paths, &original);
        let commits_before = commit_count(&paths);

        let a1 = "인공물보다 방치된 것을 고른다. 그리고 오래 머문다.";
        let b1 = "채도가 아니라 습도로 공간을 읽는다. 온도는 나중이다.";
        let edited = doc_text(
            HEADER_BODY,
            &[
                // 마커 해시는 옛 본문(a0/b0) 것이고 본문만 바뀌었다 → 두 블록 모두 dirty.
                neg_block(PROFILE_BLOCK_ID, 17, a0, a1),
                neg_block("stance", 4, b0, b1),
            ],
            "사람이 쓴 문장",
        );
        let out = save_edited(&paths, &edited).expect("저장");

        assert_eq!(out.profile_edits.len(), 2, "블록마다 관측 하나씩");
        assert_eq!(out.commits, 3, "profile_edit 2 + render 1");
        assert_eq!(commit_count(&paths), commits_before + 3);

        let msgs = git::log_messages(&paths.soul(), 3).expect("git log");
        assert!(msgs[0].starts_with("render "), "{msgs:?}");
        assert_eq!(
            msgs[1..]
                .iter()
                .filter(|m| m.starts_with("profile_edit "))
                .count(),
            2,
            "관측 커밋 메시지는 `<type> <ULID>` 형식이다 (§R8): {msgs:?}"
        );

        let set = Store::new(paths.clone()).load_set().expect("관측 재적재");
        let edits = set.profile_edits();
        assert_eq!(edits.len(), 2);
        let blocks: Vec<&str> = edits.iter().map(|e| e.block.as_str()).collect();
        assert!(
            blocks.contains(&PROFILE_BLOCK_ID) && blocks.contains(&"stance"),
            "{blocks:?}"
        );
        for e in &edits {
            assert_eq!(e.author, AUTHOR_HUMAN);
            // from_hash는 마커가 주장하던 **옛** 해시다.
            assert!(e.from_hash == block_hash(a0) || e.from_hash == block_hash(b0));
        }
        let texts: Vec<&str> = edits.iter().map(|e| e.to_text.as_str()).collect();
        assert!(texts.iter().any(|t| t.contains("오래 머문다")), "{texts:?}");
        assert!(
            texts.iter().any(|t| t.contains("온도는 나중이다")),
            "{texts:?}"
        );
    }

    /// T3 — `soul:neg` 편집 → `profile_edit` 1건, `rev` +1, `hash` 갱신.
    #[test]
    fn t3_neg_edit_bumps_rev_and_refreshes_hash() {
        let (_tmp, paths) = setup("t3");
        seed_ingest(&paths);
        let old = "인공물보다 방치된 것을 고른다.";
        write_soul_md(&paths, &simple_doc(old, old, 17, "사람 문장"));

        let new = "인공물보다 방치된 것을 고른다. 사람이 방금 나간 자리에 멈춘다.";
        let out = save_edited(&paths, &simple_doc(old, new, 17, "사람 문장")).expect("저장");
        assert_eq!(out.profile_edits.len(), 1);

        let saved = parse(&read_soul_md(&paths)).expect("재파싱");
        let b = saved.block(PROFILE_BLOCK_ID).expect("profile 블록");
        assert_eq!(b.rev, Some(18), "rev가 +1 되어야 한다");
        assert_eq!(
            b.hash.as_deref(),
            Some(b.actual_hash().as_str()),
            "마커 해시가 새 본문의 해시로 갱신되어야 한다"
        );
        assert!(!b.is_dirty(), "저장 직후 문서는 dirty가 아니다");
        assert!(saved.human_body().unwrap_or("").contains("사람 문장"));
    }

    /// T5 — `soul:gen` 편집 → 경고용으로 블록 id를 돌려준다.
    #[test]
    fn t5_edited_gen_block_is_reported() {
        let (_tmp, paths) = setup("t5");
        seed_ingest(&paths);
        let profile = "인공물보다 방치된 것을 고른다.";
        write_soul_md(&paths, &simple_doc(profile, profile, 1, "사람 문장"));

        let edited = doc_text(
            "사용자가 손으로 고쳐 쓴 헤더",
            &[neg_block(PROFILE_BLOCK_ID, 1, profile, profile)],
            "사람 문장",
        );
        let out = save_edited(&paths, &edited).expect("저장");
        assert_eq!(out.gen_blocks_modified, vec!["header".to_string()]);
        assert!(
            out.profile_edits.is_empty(),
            "gen 편집은 관측을 만들지 않는다"
        );
        // 경고만 하고 저장은 그대로 진행한다 (§8.3 규칙 7).
        assert_eq!(out.commits, 1);
        assert!(
            !read_soul_md(&paths).contains("사용자가 손으로 고쳐 쓴 헤더"),
            "gen 블록은 파생값으로 덮어써진다"
        );
    }

    /// §R2 — `SOUL.md`는 관측 로그의 투영이다. 저장 경로가 만든 파일과
    /// 재빌드 경로가 만드는 파일이 다르면 편집 직후 `soul rebuild`가 문서를 흔든다.
    ///
    /// 특히 `rev`: 저장은 "마커의 rev +1"(§8.3 규칙 4)로, 재빌드는
    /// "profile을 건드린 관측의 누적 수"로 구한다. 로그와 파일이 어긋나지 않는 한
    /// 두 값은 같아야 하며, 이 테스트가 그것을 고정한다.
    #[test]
    fn saved_file_is_what_rebuild_would_render() {
        let (_tmp, paths) = setup("projection");
        seed_ingest(&paths);
        // 관측이 없는 초기 상태: profile 본문은 비어 있고 rev는 0이다.
        write_soul_md(&paths, &simple_doc("", "", 0, "사람 문장"));

        let new = "인공물보다 방치된 것을 고른다.";
        let out = save_edited(&paths, &simple_doc("", new, 0, "사람 문장")).expect("저장");
        assert_eq!(out.profile_edits.len(), 1);
        let after_save = read_soul_md(&paths);

        let (derived, _missing) = crate::rebuild::replay(&paths, false).expect("재생");
        let report = crate::rebuild::render_soul_md(&paths, &derived, "rebuild 2").expect("재렌더");
        assert!(
            !report.soul_md_changed,
            "재빌드가 저장 결과를 바꾸면 안 된다:\n--- 저장 ---\n{after_save}\n--- 재빌드 ---\n{}",
            read_soul_md(&paths)
        );
        assert_eq!(read_soul_md(&paths), after_save);
    }

    /// §8.4 1단계 — 락 실패 시 즉시 중단하고 아무것도 쓰지 않는다.
    #[test]
    fn lock_failure_aborts_before_touching_anything() {
        let (_tmp, paths) = setup("lock");
        seed_ingest(&paths);
        let profile = "인공물보다 방치된 것을 고른다.";
        let original = simple_doc(profile, profile, 1, "사람 문장");
        write_soul_md(&paths, &original);
        let commits_before = commit_count(&paths);
        let obs_before = obs_count(&paths);

        let held = WriteLock::acquire(&paths.soul()).expect("첫 락은 잡힌다");
        let err = save_edited(&paths, &simple_doc(profile, "고친 본문", 1, "사람 문장"))
            .expect_err("락이 잡혀 있으면 저장은 실패한다");
        assert!(matches!(err, SoulError::LockBusy { .. }), "{err:?}");
        assert_eq!(read_soul_md(&paths), original, "파일이 변하면 안 된다");
        assert_eq!(commit_count(&paths), commits_before);
        assert_eq!(obs_count(&paths), obs_before);
        drop(held);
    }

    /// §18-5 — 저장도 `SOUL.md`를 **원자적으로** 쓴다.
    ///
    /// 제자리 덮어쓰기는 파일을 먼저 0바이트로 자른다. 그 창에서 프로세스가 죽으면
    /// 관측 로그로 복원할 수 없는 `soul:human`이 통째로 사라진다.
    /// 하드 링크로 옛 inode를 붙잡아 두면 `rename`인지 제자리 덮어쓰기인지 구별된다.
    #[cfg(unix)]
    #[test]
    fn save_replaces_soul_md_atomically() {
        let (_tmp, paths) = setup("atomic-save");
        seed_ingest(&paths);
        let profile = "인공물보다 방치된 것을 고른다.";
        let original = simple_doc(profile, profile, 1, "예전에 쓴 문장");
        write_soul_md(&paths, &original);
        let old_inode = paths.soul().join("old-inode");
        std::fs::hard_link(paths.soul_md(), &old_inode).expect("하드 링크");

        save_edited(&paths, &simple_doc(profile, profile, 1, "새로 쓴 문장")).expect("저장");

        assert_eq!(
            std::fs::read_to_string(&old_inode).expect("옛 inode"),
            original,
            "제자리 덮어쓰기는 0바이트 창을 만든다 — 임시 파일 + rename 이어야 한다"
        );
        assert!(read_soul_md(&paths).contains("새로 쓴 문장"));
    }

    /// 임시 파일이 남으면 `git status`가 더러워지고 다음 `commit_all`에 딸려 들어간다 (T20).
    #[test]
    fn atomic_write_leaves_no_temp_file() {
        let (_tmp, paths) = setup("atomic-temp");
        write_soul_md_atomic(&paths.soul_md(), "# SOUL\n").expect("원자적 쓰기");
        assert_eq!(read_soul_md(&paths), "# SOUL\n");
        assert!(
            !paths.soul().join(SOUL_MD_TMP).exists(),
            "임시 파일은 rename 으로 사라져야 한다"
        );
        // 이름이 바뀌어도 잡히도록 디렉토리 전체를 훑는다. `.gitignore`에 없는 찌꺼기가
        // 남으면 `git status`에 뜨고 `commit_all`에 딸려 들어간다 (T20).
        let leftovers: Vec<String> = std::fs::read_dir(paths.soul())
            .expect("디렉토리 읽기")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    /// T10 — 마커 짝 불일치 → 파싱 실패, 파일 미변경. 되돌리지도 않는다.
    #[test]
    fn t10_unbalanced_markers_abort_without_writing() {
        let (_tmp, paths) = setup("t10");
        seed_ingest(&paths);
        let profile = "인공물보다 방치된 것을 고른다.";
        let original = simple_doc(profile, profile, 1, "사람 문장");
        write_soul_md(&paths, &original);
        let commits_before = commit_count(&paths);
        let obs_before = obs_count(&paths);

        let broken = "# SOUL\n\n<!-- soul:neg id=profile rev=1 hash=a3f91c -->\n닫는 마커가 없다\n";
        let err = save_edited(&paths, broken).expect_err("파싱 실패여야 한다");
        assert!(matches!(err, SoulError::SoulParse { .. }), "{err:?}");
        assert_eq!(read_soul_md(&paths), original);
        assert_eq!(commit_count(&paths), commits_before);
        assert_eq!(obs_count(&paths), obs_before);
    }
}
