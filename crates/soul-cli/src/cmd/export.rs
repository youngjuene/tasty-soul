//! `soul export --target=prompt` (§14 · §19.8).
//!
//! 에이전트 루프 없이 모델과 직접 대화하고 싶은 경우를 위한 **한 줄짜리 탈출구**다.
//!
//! ```text
//! soul export --target=prompt > SOUL.prompt.md
//! ```
//!
//! `SOUL.md` 원문에서 마커 주석만 제거한 것을 출력한다. **번역·요약·축약을 하지 않는다.**
//! 조립은 전부 `soulblocks::export_prompt`가 한다 — 여기서 문자열을 손대면 §19.8이 깨진다.
//!
//! 산출물은 `exports/SOUL.prompt.md`에도 남긴다 (§3 디렉토리). `exports/`는 git 밖이다.
//! 안내 문구는 **stderr**로 낸다. 위처럼 리다이렉트해도 파일이 오염되지 않아야 한다.

use anyhow::{Context, Result};
use soul_core::paths::Paths;
use soul_core::{soulblocks, soulmd};

pub fn export_prompt(paths: &Paths) -> Result<()> {
    let md_path = paths.soul_md();
    let raw = std::fs::read_to_string(&md_path)
        .with_context(|| format!("{} 을 읽을 수 없습니다", md_path.display()))?;

    // §15 — 파싱 실패면 아무것도 쓰지 않고 알린다 (T10).
    let doc = soulmd::parse(&raw)?;
    let out = soulblocks::export_prompt(&doc)?;

    let dir = paths.exports();
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join("SOUL.prompt.md");
    std::fs::write(&dest, out.as_bytes())
        .with_context(|| format!("{} 에 쓸 수 없습니다", dest.display()))?;

    print!("{out}");
    if !out.ends_with('\n') {
        println!();
    }
    eprintln!("{} 에 저장했습니다.", dest.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 빈 루트에 정상 `SOUL.md`를 하나 만들어 둔다. 손으로 쓴 마크다운을 쓰면
    /// 마커 형식이 바뀔 때마다 이 테스트가 같이 썩는다.
    fn root_with_soul_md() -> Paths {
        let root = std::env::temp_dir()
            .join("tasty-soul-cli-export")
            .join(soul_core::ids::new_id().to_string());
        let paths = Paths::at(root);
        paths.ensure_dirs().unwrap();
        crate::cmd::rebuild::render(&paths, true).unwrap();
        assert!(paths.soul_md().is_file());
        paths
    }

    fn dest(paths: &Paths) -> std::path::PathBuf {
        paths.exports().join("SOUL.prompt.md")
    }

    /// §19.8 — 마커 주석 줄만 사라지고 본문은 그대로다. 요약·축약을 하지 않는다.
    #[test]
    fn export_strips_markers_and_writes_the_file() {
        let paths = root_with_soul_md();
        let raw = std::fs::read_to_string(paths.soul_md()).unwrap();
        assert!(raw.contains("<!-- soul:gen"), "원본에는 마커가 있다");

        export_prompt(&paths).unwrap();

        let out = std::fs::read_to_string(dest(&paths)).unwrap();
        assert!(!out.contains("<!-- soul:gen"), "마커 주석 줄은 제거된다");
        assert!(!out.contains("<!-- /soul:"), "닫는 마커도 제거된다");
        // 본문은 살아 있다.
        assert!(out.contains("## 축"));
        assert!(out.contains("# SOUL"));
        // `exports/`는 git 밖이다 — `soul/` 아래에 쓰지 않는다 (§3).
        assert!(!dest(&paths).starts_with(paths.soul()));
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    /// T10 — 마커 짝이 맞지 않으면 파싱에서 멈춘다. **아무 파일도 쓰지 않는다.**
    #[test]
    fn broken_markers_fail_without_writing_anything() {
        let paths = root_with_soul_md();
        let raw = std::fs::read_to_string(paths.soul_md()).unwrap();
        // 닫는 마커 하나를 지워 짝을 깬다.
        let broken = raw.replacen("<!-- /soul:gen -->", "", 1);
        assert_ne!(broken, raw, "픽스처가 마커를 갖고 있어야 한다");
        std::fs::write(paths.soul_md(), &broken).unwrap();

        assert!(export_prompt(&paths).is_err());
        assert!(
            !dest(&paths).exists(),
            "T10 — 실패하면 산출물을 남기지 않는다"
        );
        // 원본도 건드리지 않는다.
        assert_eq!(std::fs::read_to_string(paths.soul_md()).unwrap(), broken);
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    /// `SOUL.md`가 없으면 경로를 적은 에러다. 빈 프롬프트를 만들어 내지 않는다.
    #[test]
    fn missing_soul_md_is_an_error() {
        let root = std::env::temp_dir()
            .join("tasty-soul-cli-export-missing")
            .join(soul_core::ids::new_id().to_string());
        let paths = Paths::at(&root);
        paths.ensure_dirs().unwrap();

        let e = export_prompt(&paths).unwrap_err();
        assert!(e.to_string().contains("SOUL.md"), "{e}");
        assert!(!dest(&paths).exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
