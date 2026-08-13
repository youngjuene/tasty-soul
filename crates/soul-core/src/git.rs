//! `soul/` git 저장소 (§R8 · §D1 · §20.6).
//!
//! - **remote를 설정하지 않는다** (§D1).
//! - `soul/`에 대한 **모든 쓰기가 커밋 하나**다. 배치 커밋을 하지 않는다.
//! - 작성자는 `tasty-soul <noreply@localhost>` 고정.
//! - **커밋 타임스탬프는 `now()`를 쓴다** — git 메타데이터는 파생값이 아니므로
//!   §R1의 적용 대상이 아니고, T1도 `SOUL.md` 내용만 비교한다.
//! - `gc.auto = 0`을 설정한다. 투입 중 자동 gc가 걸려 멈추는 일을 막는다 (§20.6).
//!
//! | 계기 | 커밋 메시지 |
//! |---|---|
//! | 관측 파일 추가 | `<type> <ULID>` |
//! | SOUL.md 재렌더 | `render <T_ref ISO8601>` |
//! | 재빌드 | `rebuild <관측 수>` |

use crate::error::{Result, SoulError};
use crate::{GIT_AUTHOR_EMAIL, GIT_AUTHOR_NAME};
use std::path::{Path, PathBuf};

/// `.gitattributes` — `SOUL.md`가 플랫폼에 따라 CRLF로 체크아웃되면 T1이 깨진다.
const GITATTRIBUTES_LINES: [&str; 1] = ["*.md text eol=lf"];

/// `.gitignore` — 락 파일과 성찰 대기본은 커밋 대상이 아니다 (§3, T20).
const GITIGNORE_LINES: [&str; 2] = [".write.lock", "SOUL.next.md"];

/// 저장소가 없으면 만든다 (§3 최초 실행 2단계).
///
/// `.gitattributes`(`*.md text eol=lf`)와 `.gitignore`(`.write.lock`, `SOUL.next.md`)를
/// 쓰고, `gc.auto = 0`을 설정한다.
pub fn ensure_repo(soul_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(soul_dir)?;

    // §D1 — remote 를 만들지 않는다. `init`은 remote를 만들지 않으므로 그대로 둔다.
    let repo = match git2::Repository::open(soul_dir) {
        Ok(r) => r,
        Err(_) => git2::Repository::init(soul_dir)?,
    };

    let attrs_changed = ensure_lines(&soul_dir.join(".gitattributes"), &GITATTRIBUTES_LINES)?;
    let ignore_changed = ensure_lines(&soul_dir.join(".gitignore"), &GITIGNORE_LINES)?;

    // §20.6 — 투입 중 자동 gc가 걸려 멈추는 일을 막는다. gc는 `soul maintain`이 한다.
    repo.config()?.set_str("gc.auto", "0")?;

    // 저장소 메타데이터를 **여기서 커밋한다.** 호출자에게 미루지 않는다.
    //
    // 이유 셋:
    // 1. `.gitattributes`의 `*.md text eol=lf`가 커밋되어 있지 않으면, 이 저장소를 새로
    //    체크아웃했을 때(백업 복원·`git log -p SOUL.md`로 과거를 꺼내볼 때) 규칙이 적용되지
    //    않는다. Windows에서 `SOUL.md`가 CRLF로 나오고, §8.3의 해시 정규화가 다른 바이트를
    //    보게 되며, T1의 바이트 동일성이 깨진다.
    // 2. 커밋하지 않으면 두 파일이 영원히 untracked로 남아 `git status`가 항상 더럽다.
    //    그러면 T20(`.write.lock`·`SOUL.next.md` 미노출)이 아무것도 재지 못하고
    //    `soul maintain`이 매번 헛경고를 낸다.
    // 3. `soul render`처럼 `App::open_or_init`을 거치지 않는 진입점이 있다. 초기화를
    //    호출자 쪽에 두면 **어느 명령을 먼저 실행했느냐에 따라 저장소 상태가 갈린다.**
    //    실제로 그랬다 — `soul doctor`를 먼저 쓰면 커밋되고 `soul render`를 먼저 쓰면 안 됐다.
    if attrs_changed || ignore_changed {
        commit_paths(
            soul_dir,
            &[Path::new(".gitattributes"), Path::new(".gitignore")],
            "init repo",
        )?;
    }

    Ok(())
}

/// 지정한 경로들을 스테이징하고 커밋한다. 변경이 없으면 커밋하지 않고 `Ok(None)`.
pub fn commit_paths(soul_dir: &Path, rel_paths: &[&Path], message: &str) -> Result<Option<String>> {
    let repo = git2::Repository::open(soul_dir)?;
    let mut index = repo.index()?;
    for p in rel_paths {
        let rel = relativize(soul_dir, p)?;
        if soul_dir.join(&rel).exists() {
            index.add_path(&rel)?;
        } else if index.get_path(&rel, 0).is_some() {
            // 삭제된 파일도 이 커밋에 담는다.
            index.remove_path(&rel)?;
        }
    }
    index.write()?;
    commit_index(&repo, &mut index, message)
}

/// 워킹 트리 전체를 스테이징하고 커밋한다 (`rebuild <n>` 용).
pub fn commit_all(soul_dir: &Path, message: &str) -> Result<Option<String>> {
    let repo = git2::Repository::open(soul_dir)?;
    let mut index = repo.index()?;
    // FORCE를 주지 않으므로 `.gitignore` 대상은 스테이징되지 않는다 (T20).
    index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
    // `add_all`은 사라진 파일을 지우지 않는다. 삭제 반영은 `update_all`이 한다.
    index.update_all(["*"].iter(), None)?;
    index.write()?;
    commit_index(&repo, &mut index, message)
}

/// `git status`가 비어 있는가 (T20 검증용).
pub fn is_clean(soul_dir: &Path) -> Result<bool> {
    let repo = git2::Repository::open(soul_dir)?;
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false)
        .include_unmodified(false);
    let clean = repo.statuses(Some(&mut opts))?.is_empty();
    Ok(clean)
}

/// 커밋 메시지 목록 (최신순). 테스트가 커밋 수와 형식을 검증한다 (T21).
pub fn log_messages(soul_dir: &Path, limit: usize) -> Result<Vec<String>> {
    let repo = git2::Repository::open(soul_dir)?;
    let mut out = Vec::new();
    if limit == 0 || head_commit(&repo)?.is_none() {
        return Ok(out);
    }
    let mut walk = repo.revwalk()?;
    // TOPOLOGICAL 을 함께 준다. 같은 초에 여러 커밋이 생기는 일이 흔한데(§8.4의
    // 저장 1회 = 커밋 여러 개) TIME 만으로는 동률에서 순서가 뒤집힌다.
    walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)?;
    walk.push_head()?;
    for oid in walk {
        if out.len() >= limit {
            break;
        }
        let commit = repo.find_commit(oid?)?;
        // summary는 메시지 첫 줄이다. 우리 메시지는 항상 한 줄이다 (§R8).
        out.push(commit.summary()?.unwrap_or_default().to_string());
    }
    Ok(out)
}

/// `soul maintain` (§14 · §20.6).
pub fn gc(soul_dir: &Path) -> Result<()> {
    // libgit2에는 gc가 없다. 시스템 git을 부른다.
    // `gc.auto = 0`은 `git gc --auto`만 막으므로 여기서는 그대로 압축된다.
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(soul_dir)
        .arg("gc")
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                SoulError::config("git 실행 파일을 찾을 수 없습니다. git을 설치하세요")
            }
            _ => SoulError::Io(e),
        })?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(SoulError::invalid(format!("git gc 실패: {msg}")));
    }
    Ok(())
}

// ───────────────────────────────────────────────────────────────── 내부 도우미

/// 인덱스를 트리로 굳혀 커밋한다. 부모가 없는 최초 커밋도 처리한다.
fn commit_index(
    repo: &git2::Repository,
    index: &mut git2::Index,
    message: &str,
) -> Result<Option<String>> {
    let tree_oid = index.write_tree()?;
    let parent = head_commit(repo)?;

    // 변경이 없으면 커밋하지 않는다 — 빈 커밋은 `git log`를 성장 타임라인이 아니게 만든다.
    match &parent {
        Some(p) if p.tree_id() == tree_oid => return Ok(None),
        None if repo.find_tree(tree_oid)?.is_empty() => return Ok(None),
        _ => {}
    }

    let tree = repo.find_tree(tree_oid)?;
    // §R8 — 작성자·커미터 고정, 타임스탬프는 now().
    let sig = git2::Signature::now(GIT_AUTHOR_NAME, GIT_AUTHOR_EMAIL)?;
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    let oid = repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)?;
    Ok(Some(oid.to_string()))
}

/// HEAD 커밋. 저장소가 비어 있으면(최초 커밋 전) `None`.
fn head_commit(repo: &git2::Repository) -> Result<Option<git2::Commit<'_>>> {
    match repo.head() {
        Ok(h) => Ok(Some(h.peel_to_commit()?)),
        Err(e)
            if matches!(
                e.code(),
                git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound
            ) =>
        {
            Ok(None)
        }
        Err(e) => Err(e.into()),
    }
}

/// 저장소 기준 상대 경로로 만든다. 절대 경로도 받는다.
fn relativize(soul_dir: &Path, p: &Path) -> Result<PathBuf> {
    if p.is_relative() {
        return Ok(p.to_path_buf());
    }
    if let Ok(rel) = p.strip_prefix(soul_dir) {
        return Ok(rel.to_path_buf());
    }
    // 심볼릭 링크(macOS의 /tmp 등)로 접두사가 어긋나는 경우를 한 번 더 본다.
    let (base, full) = (soul_dir.canonicalize()?, p.canonicalize()?);
    full.strip_prefix(&base)
        .map(|r| r.to_path_buf())
        .map_err(|_| SoulError::MissingPath(p.to_path_buf()))
}

/// 파일에 필요한 줄이 모두 있게 한다. 이미 있으면 건드리지 않고,
/// 파일이 있는데 줄이 빠져 있으면 뒤에 덧붙인다 (사용자가 추가한 줄을 지우지 않는다).
/// 필요한 줄이 없으면 덧붙인다. 파일을 덮어쓰지 않는다 — 사용자가 추가한 줄을 지키기 위해서다.
/// 반환값은 **파일을 실제로 건드렸는가** (호출자가 커밋 여부를 정한다).
fn ensure_lines(path: &Path, required: &[&str]) -> Result<bool> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let missing: Vec<&str> = required
        .iter()
        .copied()
        .filter(|line| !existing.lines().any(|l| l.trim() == *line))
        .collect();
    if missing.is_empty() && path.exists() {
        return Ok(false);
    }
    let mut out = existing;
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    for line in missing {
        out.push_str(line);
        out.push('\n');
    }
    std::fs::write(path, out.as_bytes())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obs::model::*;
    use crate::obs::store::Store;
    use crate::paths::Paths;
    use crate::time::Ts;
    use crate::SCHEMA_VERSION;

    fn temp_paths(tag: &str) -> Paths {
        let root = std::env::temp_dir().join(format!("tasty-soul-{tag}-{}", crate::ids::new_id()));
        let paths = Paths::at(root);
        paths.ensure_dirs().expect("디렉토리 생성");
        paths
    }

    /// 저장소 + 최초 커밋(§3 최초 실행 3단계)까지 만든 상태.
    fn repo_with_skeleton(tag: &str) -> Paths {
        let paths = temp_paths(tag);
        ensure_repo(&paths.soul()).expect("ensure_repo");
        std::fs::write(paths.soul_md(), "# SOUL\n").expect("SOUL.md 스켈레톤");
        commit_all(&paths.soul(), "render 최초").expect("최초 커밋");
        paths
    }

    fn sample_ingest(ts: &str) -> Observation {
        Observation::Ingest(Ingest {
            id: crate::ids::new_id(),
            ts: Ts::parse(ts).expect("ts"),
            schema: SCHEMA_VERSION,
            source: Source {
                kind: Kind::Image,
                sha256: "abcd1234".into(),
                origin: "file:///Users/x/a.jpg".into(),
                bytes: 1024,
                mime: "image/jpeg".into(),
            },
            machine: Machine {
                prose: "차갑고 정돈된 실내".into(),
                axes: Axes::ZERO,
                tags: vec!["실내".into()],
                quality: Quality::Full,
                prompt_sha256: "9f2c1a".into(),
            },
            min_dist: None,
            surprisal: 0.5,
            model: ModelRef {
                provider: "openai".into(),
                id: "gpt-x".into(),
                prompt_sha256: None,
                calls: vec![],
            },
            supersedes: None,
        })
    }

    #[test]
    fn ensure_repo_is_idempotent_and_has_no_remote() {
        let paths = temp_paths("git-init");
        let soul = paths.soul();
        ensure_repo(&soul).expect("첫 호출");
        ensure_repo(&soul).expect("두 번째 호출도 안전해야 한다");

        let repo = git2::Repository::open(&soul).expect("저장소 열기");
        // §D1 — remote 를 만들지 않는다.
        assert!(
            repo.remotes().expect("remotes").is_empty(),
            "remote가 없어야 한다"
        );
        // §20.6 — gc.auto = 0.
        assert_eq!(
            repo.config().unwrap().get_string("gc.auto").ok().as_deref(),
            Some("0")
        );
        assert_eq!(
            std::fs::read_to_string(soul.join(".gitattributes")).unwrap(),
            "*.md text eol=lf\n"
        );
        assert_eq!(
            std::fs::read_to_string(soul.join(".gitignore")).unwrap(),
            ".write.lock\nSOUL.next.md\n"
        );
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn t20_lock_and_next_md_do_not_show_in_status() {
        // T20 — .write.lock 과 SOUL.next.md 는 git status 에 나타나지 않는다.
        let paths = repo_with_skeleton("git-t20");
        let soul = paths.soul();
        assert!(is_clean(&soul).unwrap(), "최초 커밋 직후에는 깨끗해야 한다");

        std::fs::write(paths.write_lock(), "4242").unwrap();
        std::fs::write(paths.soul_next_md(), "# 제안\n").unwrap();
        assert!(
            is_clean(&soul).unwrap(),
            ".write.lock / SOUL.next.md 는 status 에 뜨면 안 된다"
        );

        // 대조군: 무시 대상이 아닌 파일은 status 에 떠야 한다 (is_clean 이 항상 true 가 아님).
        std::fs::write(soul.join("NOTES.md"), "x").unwrap();
        assert!(!is_clean(&soul).unwrap());

        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn t21_one_observation_makes_one_commit() {
        // T21 — 관측 1건 추가 → 커밋 1개 증가, 메시지는 `<type> <ULID>`.
        let paths = repo_with_skeleton("git-t21");
        let soul = paths.soul();
        let before = log_messages(&soul, 100).unwrap().len();

        let obs = sample_ingest("2026-08-13T09:12:33.123Z");
        let store = Store::new(paths.clone());
        let file = store.append(&obs).expect("관측 기록");
        let rel = file.strip_prefix(&soul).expect("soul 하위 경로");

        let message = format!("{} {}", obs.type_name(), obs.id());
        let oid = commit_paths(&soul, &[rel], &message)
            .expect("커밋")
            .expect("커밋 생성됨");
        assert_eq!(oid.len(), 40, "커밋 해시를 돌려줘야 한다");

        let log = log_messages(&soul, 100).unwrap();
        assert_eq!(log.len(), before + 1, "커밋이 정확히 1개 늘어야 한다");
        assert_eq!(log[0], message);
        assert!(log[0].starts_with("ingest "));
        assert!(is_clean(&soul).unwrap(), "커밋 후 워킹 트리는 깨끗하다");

        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn commit_author_is_fixed() {
        let paths = repo_with_skeleton("git-author");
        let repo = git2::Repository::open(paths.soul()).unwrap();
        let head = head_commit(&repo).unwrap().expect("최초 커밋");
        assert_eq!(head.author().name().ok(), Some(GIT_AUTHOR_NAME));
        assert_eq!(head.author().email().ok(), Some(GIT_AUTHOR_EMAIL));
        assert_eq!(head.committer().name().ok(), Some(GIT_AUTHOR_NAME));
        assert_eq!(head.committer().email().ok(), Some(GIT_AUTHOR_EMAIL));
        // `ensure_repo`가 만든 `init repo`가 최초 커밋이고 그 위에 스켈레톤 커밋이 얹힌다.
        let root = {
            let mut c = head.clone();
            while c.parent_count() > 0 {
                c = c.parent(0).unwrap();
            }
            c
        };
        assert_eq!(root.parent_count(), 0, "최초 커밋은 부모가 없다");
        assert_eq!(root.message().ok().map(str::trim), Some("init repo"));
        assert_eq!(root.author().name().ok(), Some(GIT_AUTHOR_NAME));
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn commit_without_changes_returns_none() {
        let paths = repo_with_skeleton("git-nochange");
        let soul = paths.soul();
        assert!(commit_all(&soul, "render 변화 없음").unwrap().is_none());
        assert!(
            commit_paths(&soul, &[Path::new("SOUL.md")], "render 변화 없음")
                .unwrap()
                .is_none()
        );
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn ensure_repo_commits_its_metadata_so_no_entry_point_leaves_it_untracked() {
        // `.gitattributes`(`*.md text eol=lf`)가 커밋되지 않으면 새 체크아웃에서 규칙이
        // 적용되지 않아 Windows에서 CRLF가 되고 T1이 깨진다. 그리고 두 파일이 untracked로
        // 남으면 `git status`가 항상 더러워 T20이 아무것도 재지 못한다.
        //
        // 이 커밋을 호출자에게 미루면 **어느 명령을 먼저 실행했느냐로 저장소 상태가 갈린다**
        // (`soul doctor` 먼저면 커밋되고 `soul render` 먼저면 안 됐다).
        let paths = temp_paths("git-empty");
        let soul = paths.soul();
        ensure_repo(&soul).unwrap();
        assert_eq!(
            log_messages(&soul, 10).unwrap(),
            vec!["init repo".to_string()],
            "ensure_repo가 메타데이터를 커밋해야 한다"
        );
        assert!(is_clean(&soul).unwrap(), "초기화 직후 작업 트리는 깨끗하다");

        // 두 번째 호출은 아무것도 바꾸지 않으므로 커밋도 늘지 않는다 (멱등).
        ensure_repo(&soul).unwrap();
        assert_eq!(log_messages(&soul, 10).unwrap().len(), 1);

        std::fs::write(paths.soul_md(), "# SOUL\n").unwrap();
        let oid = commit_all(&soul, "render 최초").unwrap();
        assert!(oid.is_some());
        assert_eq!(
            log_messages(&soul, 10).unwrap(),
            vec!["render 최초".to_string(), "init repo".to_string()]
        );
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn commit_paths_accepts_absolute_paths_and_records_deletion() {
        let paths = repo_with_skeleton("git-abs");
        let soul = paths.soul();
        let f = soul.join("A.md");
        std::fs::write(&f, "a\n").unwrap();
        assert!(commit_paths(&soul, &[&f], "render 추가").unwrap().is_some());
        assert!(is_clean(&soul).unwrap());

        std::fs::remove_file(&f).unwrap();
        assert!(commit_paths(&soul, &[&f], "render 삭제").unwrap().is_some());
        assert!(is_clean(&soul).unwrap(), "삭제도 커밋에 담겨야 한다");
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn log_respects_limit_and_is_newest_first() {
        let paths = repo_with_skeleton("git-log");
        let soul = paths.soul();
        for i in 0..3 {
            std::fs::write(soul.join(format!("f{i}.md")), format!("{i}\n")).unwrap();
            commit_all(&soul, &format!("render {i}")).unwrap();
        }
        let log = log_messages(&soul, 2).unwrap();
        assert_eq!(log, vec!["render 2".to_string(), "render 1".to_string()]);
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn gc_leaves_history_intact() {
        let paths = repo_with_skeleton("git-gc");
        let soul = paths.soul();
        // git 실행 파일이 없는 환경에서는 검증을 건너뛴다.
        if std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        gc(&soul).expect("gc");
        assert_eq!(
            log_messages(&soul, 10).unwrap(),
            vec!["render 최초".to_string(), "init repo".to_string()]
        );
        let _ = std::fs::remove_dir_all(&paths.root);
    }
}
