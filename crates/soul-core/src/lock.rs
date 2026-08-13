//! 단일 writer 보장 (§R7).
//!
//! `soul/.write.lock`에 pid를 쓰고 flock을 건다. Tauri single-instance 플러그인도 함께 쓴다.
//! **락 획득 실패 시 쓰기 작업은 즉시 에러다. 대기하지 않는다** (§15).
//! 이 파일은 커밋 대상이 아니다 — `.gitignore`에 들어간다 (T20).

use crate::error::{Result, SoulError};
use std::io::Write;
use std::path::Path;

/// 락 파일 이름. `Paths::write_lock()`과 같은 값이다.
const LOCK_FILE_NAME: &str = ".write.lock";

/// 살아 있는 동안 락을 보유한다. drop 시 해제된다.
pub struct WriteLock {
    _file: std::fs::File,
    path: std::path::PathBuf,
}

impl WriteLock {
    /// 비차단으로 시도한다. 이미 잠겨 있으면 `SoulError::LockBusy`.
    pub fn acquire(soul_dir: &Path) -> Result<WriteLock> {
        std::fs::create_dir_all(soul_dir)?;
        let path = soul_dir.join(LOCK_FILE_NAME);

        // 락을 잡기 **전에** 기존 내용을 읽어 둔다. 실패했을 때 누가 쥐고 있는지
        // 알려주기 위한 것이며, 이 시점 이후에는 우리가 파일을 덮어쓴다.
        let holder = read_pid(&path);

        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;

        // 비차단 배타 락. **대기하지 않는다** (§R7).
        // 락을 먼저 잡고 pid를 쓴다 — 순서가 반대면 실패한 프로세스가
        // 락 보유자의 pid를 지워 버린다.
        match fs4::FileExt::try_lock(&file) {
            Ok(()) => {}
            Err(fs4::TryLockError::WouldBlock) => return Err(SoulError::LockBusy { holder }),
            Err(fs4::TryLockError::Error(e)) => return Err(SoulError::Io(e)),
        }

        // 잔존 pid를 지우고 현재 pid만 남긴다. 방금 연 핸들이므로 커서는 0이다.
        let mut file = file;
        file.set_len(0)?;
        write!(file, "{}", std::process::id())?;
        file.flush()?;

        Ok(WriteLock { _file: file, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for WriteLock {
    fn drop(&mut self) {
        // flock은 파일 핸들이 닫히면 해제된다. 파일 자체는 남겨 둔다
        // (경로가 .gitignore에 있으므로 무해하고, 잔존 pid는 진단에 쓸모가 있다).
    }
}

/// 락 파일에 남아 있는 pid. 파일이 없거나 숫자가 아니면 `None`.
fn read_pid(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("tasty-soul-{tag}-{}", crate::ids::new_id()));
        std::fs::create_dir_all(&p).expect("임시 디렉토리 생성");
        p
    }

    #[test]
    fn acquire_writes_current_pid() {
        let dir = temp_dir("lock-pid");
        let lock = WriteLock::acquire(&dir).expect("락 획득");
        assert_eq!(lock.path(), dir.join(".write.lock"));
        assert_eq!(read_pid(lock.path()), Some(std::process::id()));
        drop(lock);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn second_acquire_fails_immediately() {
        // §R7 — 두 번째 시도는 대기하지 않고 즉시 LockBusy 여야 한다.
        let dir = temp_dir("lock-busy");
        let held = WriteLock::acquire(&dir).expect("첫 락 획득");

        let started = std::time::Instant::now();
        let second = WriteLock::acquire(&dir);
        let elapsed = started.elapsed();

        match second {
            Err(SoulError::LockBusy { holder }) => {
                assert_eq!(holder, Some(std::process::id()), "보유자 pid를 알려야 한다");
            }
            Err(e) => panic!("LockBusy 를 기대했다: {e}"),
            Ok(_) => panic!("이미 잠긴 락을 두 번째로 잡으면 안 된다"),
        }
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "비차단이어야 한다 (걸린 시간 {elapsed:?})"
        );

        drop(held);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lock_is_released_on_drop() {
        let dir = temp_dir("lock-drop");
        let first = WriteLock::acquire(&dir).expect("첫 락 획득");
        drop(first);
        let second = WriteLock::acquire(&dir);
        assert!(second.is_ok(), "drop 후에는 다시 잡을 수 있어야 한다");
        drop(second);
        // 파일은 남는다 (진단용).
        assert!(dir.join(".write.lock").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn acquire_creates_missing_soul_dir() {
        let dir = temp_dir("lock-mkdir").join("soul");
        let lock = WriteLock::acquire(&dir).expect("디렉토리가 없어도 만들어야 한다");
        assert!(dir.is_dir());
        drop(lock);
        if let Some(parent) = dir.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }
}
