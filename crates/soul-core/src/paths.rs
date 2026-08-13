//! 앱 데이터 루트와 하위 경로 (§3).
//!
//! macOS `~/Library/Application Support/tasty-soul/`, Windows `%APPDATA%\tasty-soul\`.
//! `SOUL_ROOT` 환경변수로 덮어쓸 수 있다 — 테스트와 픽스처가 이것에 의존한다.

use crate::error::Result;
use crate::time::Ts;
use std::path::{Path, PathBuf};

pub const APP_DIR_NAME: &str = "tasty-soul";
pub const ROOT_ENV: &str = "SOUL_ROOT";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    pub root: PathBuf,
}

impl Paths {
    /// 환경변수 → OS 기본 경로 순으로 해석한다.
    pub fn discover() -> Result<Paths> {
        if let Ok(p) = std::env::var(ROOT_ENV) {
            if !p.is_empty() {
                return Ok(Paths::at(PathBuf::from(p)));
            }
        }
        let base = dirs::data_dir().ok_or_else(|| {
            crate::error::SoulError::config("앱 데이터 디렉토리를 찾을 수 없습니다")
        })?;
        Ok(Paths::at(base.join(APP_DIR_NAME)))
    }

    pub fn at(root: impl Into<PathBuf>) -> Paths {
        Paths { root: root.into() }
    }

    pub fn config_toml(&self) -> PathBuf {
        self.root.join("config.toml")
    }
    /// 조달된 ffmpeg (§9.7).
    pub fn bin(&self) -> PathBuf {
        self.root.join("bin")
    }
    /// 독립 git 저장소. **remote 없음** (§D1).
    pub fn soul(&self) -> PathBuf {
        self.root.join("soul")
    }
    pub fn soul_md(&self) -> PathBuf {
        self.soul().join("SOUL.md")
    }
    /// 성찰 제안 대기본. **커밋 대상 아님** (§3).
    pub fn soul_next_md(&self) -> PathBuf {
        self.soul().join("SOUL.next.md")
    }
    pub fn write_lock(&self) -> PathBuf {
        self.soul().join(".write.lock")
    }
    pub fn observations(&self) -> PathBuf {
        self.soul().join("observations")
    }
    /// 월별 샤딩 (§20.6).
    pub fn observation_dir(&self, ts: Ts) -> PathBuf {
        self.observations().join(ts.month().to_string())
    }
    pub fn observation_file(&self, ts: Ts, id: &crate::ids::ObsId) -> PathBuf {
        self.observation_dir(ts).join(format!("{id}.json"))
    }
    /// 삭제해도 무방 (§3).
    pub fn cache(&self) -> PathBuf {
        self.root.join("cache")
    }
    pub fn derived_db(&self) -> PathBuf {
        self.cache().join("derived.sqlite")
    }
    pub fn thumbs(&self) -> PathBuf {
        self.cache().join("thumbs")
    }
    /// `thumbs/ab/cd/abcd....jpg` (§3).
    pub fn thumb_file(&self, sha256_hex: &str) -> PathBuf {
        let a = &sha256_hex[0..2.min(sha256_hex.len())];
        let b = &sha256_hex[2..4.min(sha256_hex.len())];
        self.thumbs()
            .join(a)
            .join(b)
            .join(format!("{sha256_hex}.jpg"))
    }
    /// YouTube 클립 캐시. `soul recast`가 yt-dlp를 다시 부르지 않게 한다 (§9.3, T11j).
    pub fn media_cache(&self) -> PathBuf {
        self.cache().join("media")
    }
    /// git 밖 (§3).
    pub fn exports(&self) -> PathBuf {
        self.root.join("exports")
    }
    /// 에이전트 트레이스. 삭제 가능 (§11.3).
    pub fn runs(&self) -> PathBuf {
        self.root.join("runs")
    }

    /// 필요한 디렉토리를 전부 만든다. git init은 `git::ensure_repo`가 한다.
    pub fn ensure_dirs(&self) -> Result<()> {
        for d in [
            self.root.clone(),
            self.bin(),
            self.soul(),
            self.observations(),
            self.cache(),
            self.thumbs(),
            self.media_cache(),
            self.exports(),
            self.runs(),
        ] {
            std::fs::create_dir_all(&d)?;
        }
        Ok(())
    }
}

/// 번들된 `prompts/` 디렉토리 (§R11).
///
/// 개발 중에는 워크스페이스 루트의 `prompts/`, 배포본에서는 실행 파일 옆의
/// `prompts/` 또는 Tauri 리소스 디렉토리를 쓴다. `SOUL_PROMPTS_DIR`로 덮어쓴다.
pub fn prompts_dir() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("SOUL_PROMPTS_DIR") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("prompts"));
            candidates.push(dir.join("../Resources/prompts"));
        }
    }
    // 개발 트리: crates/*/ 에서 위로 올라가며 찾는다.
    let mut cur: Option<&Path> = Some(Path::new(env!("CARGO_MANIFEST_DIR")));
    while let Some(d) = cur {
        candidates.push(d.join("prompts"));
        cur = d.parent();
    }
    candidates.into_iter().find(|p| p.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_spec() {
        let p = Paths::at("/tmp/ts");
        assert_eq!(p.soul_md(), Path::new("/tmp/ts/soul/SOUL.md"));
        assert_eq!(p.derived_db(), Path::new("/tmp/ts/cache/derived.sqlite"));
        assert_eq!(
            p.thumb_file("abcd1234ef"),
            Path::new("/tmp/ts/cache/thumbs/ab/cd/abcd1234ef.jpg")
        );
        let ts = Ts::parse("2026-08-13T09:12:33.123Z").unwrap();
        assert_eq!(
            p.observation_dir(ts),
            Path::new("/tmp/ts/soul/observations/2026-08")
        );
    }

    #[test]
    fn prompts_dir_is_found_in_dev_tree() {
        assert!(
            prompts_dir().is_some(),
            "워크스페이스의 prompts/ 를 찾아야 한다"
        );
    }
}
