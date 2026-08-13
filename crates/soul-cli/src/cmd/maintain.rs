//! `soul maintain` · `soul trace purge` (§14 · §20.6 · §11.3).
//!
//! `soul/`에는 `gc.auto = 0`이 설정되어 있다 (§20.6). 투입 중에 자동 gc가 걸려 멈추는 일을
//! 막기 위해서이고, 그래서 **압축은 사용자가 이 명령으로 부를 때만** 일어난다.
//!
//! `soul trace purge`는 `runs/`만 비운다. **관측과 `SOUL.md`는 건드리지 않는다** (T71).

use anyhow::Result;
use soul_core::paths::Paths;
use soul_core::{git, trace};

pub fn maintain(paths: &Paths) -> Result<()> {
    let soul_dir = paths.soul();
    if !soul_dir.join(".git").exists() {
        println!(
            "{} 에 git 저장소가 없습니다. 할 일이 없습니다.",
            soul_dir.display()
        );
        return Ok(());
    }

    let before = dir_size(&soul_dir.join(".git"));
    git::gc(&soul_dir)?;
    let after = dir_size(&soul_dir.join(".git"));
    println!(
        "git gc 완료 — .git {} → {}",
        human_bytes(before),
        human_bytes(after)
    );

    // 정리 후 상태를 함께 알린다. `.write.lock`·`SOUL.next.md`는 무시 대상이므로
    // 여기 나타나면 안 된다 (T20).
    match git::is_clean(&soul_dir) {
        Ok(true) => println!("작업 트리 깨끗함"),
        Ok(false) => println!("작업 트리에 커밋되지 않은 변경이 있습니다"),
        Err(e) => eprintln!("경고: git status 확인 실패: {e}"),
    }
    Ok(())
}

pub fn trace_purge(paths: &Paths) -> Result<()> {
    let n = trace::purge(paths)?;
    println!(
        "{} 에서 {n}개 항목을 지웠습니다. 관측과 SOUL.md는 그대로입니다.",
        paths.runs().display()
    );
    Ok(())
}

/// 재귀 합계. 실패하면 0으로 본다 — 정리 명령이 크기 계산 때문에 실패하면 안 된다.
fn dir_size(dir: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0u64;
    for e in entries.flatten() {
        match e.metadata() {
            Ok(m) if m.is_dir() => total += dir_size(&e.path()),
            Ok(m) => total += m.len(),
            Err(_) => {}
        }
    }
    total
}

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i + 1 < UNITS.len() {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_are_readable() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(2048), "2.0 KB");
        assert_eq!(human_bytes(3 * 1024 * 1024), "3.0 MB");
    }

    #[test]
    fn dir_size_of_missing_dir_is_zero() {
        assert_eq!(dir_size(std::path::Path::new("/이런/경로는/없다")), 0);
    }

    /// T71 — `runs/`만 비운다.
    #[test]
    fn trace_purge_leaves_observations_and_soul_md() {
        let root = std::env::temp_dir()
            .join("tasty-soul-cli-purge")
            .join(soul_core::ids::new_id().to_string());
        let paths = Paths::at(&root);
        paths.ensure_dirs().unwrap();
        std::fs::write(paths.runs().join("2026-08-13T09-12-33Z.jsonl"), b"{}\n").unwrap();
        std::fs::write(paths.soul_md(), b"# SOUL\n").unwrap();
        let obs_dir = paths.observations().join("2026-08");
        std::fs::create_dir_all(&obs_dir).unwrap();
        std::fs::write(obs_dir.join("x.json"), b"{}").unwrap();

        trace_purge(&paths).unwrap();

        assert_eq!(std::fs::read_dir(paths.runs()).unwrap().count(), 0);
        assert!(paths.runs().is_dir(), "디렉토리 자체는 남긴다");
        assert!(paths.soul_md().is_file());
        assert!(obs_dir.join("x.json").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }
}
