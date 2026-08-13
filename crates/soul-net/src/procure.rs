//! ffmpeg 조달 (§9.7).
//!
//! 1. PATH를 먼저 본다 (`soul-media::ffmpeg::locate`)
//! 2. 없으면 최초 오디오/영상 투입 시 **사용자에게 알리고** 정적 빌드를 `<root>/bin/`에 다운로드
//! 3. **SHA-256 검증 실패 시 삭제 후 중단**
//! 4. 실행 파일에 번들하지 않는다 (70MB) — §20.8
//!
//! ## 해시를 지어내지 않는다 — 지금은 조달이 막혀 있다
//!
//! 아래 표의 URL은 **최신본을 가리키는 rolling 주소**다. 내용이 바뀌면 해시도 바뀌므로
//! 소스에 박아둘 수 있는 값이 존재하지 않는다. 그래서 `sha256`을 **빈 문자열로 둔다.**
//!
//! `download_verified`는 빈 해시를 보면 다운로드를 **거부한다.** 검증 없는 실행 파일을
//! 내려받아 실행시키는 것보다, "직접 설치해 달라"고 말하는 편이 낫다.
//!
//! 조달을 켜려면 **버전이 고정된 URL**로 바꾸고 그 파일의 SHA-256을 실제로 계산해 채워라.
//! 그 전까지 §9.7 흐름은 "PATH에 있으면 쓰고, 없으면 안내"까지만 동작한다.

use sha2::{Digest, Sha256};
use soul_core::error::{Result, SoulError};
use std::path::Path;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// 조달 파일 상한. 정적 빌드는 100MB를 넘지 않는다. 해시가 틀린 거대 응답으로
/// 디스크를 채우지 못하게 막는다.
const MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct ProcureTarget {
    pub url: String,
    pub sha256: String,
    pub archive_member: Option<String>,
}

/// 현재 플랫폼에 맞는 ffmpeg/ffprobe 정적 빌드 정보.
pub fn ffmpeg_target() -> Option<(ProcureTarget, ProcureTarget)> {
    ffmpeg_target_for(std::env::consts::OS, std::env::consts::ARCH)
}

/// 플랫폼 표. `ffmpeg_target`이 현재 플랫폼으로 이걸 부른다.
///
/// `sha256`이 비어 있는 한 `download_verified`는 거부한다 — 위 모듈 주석 참조.
pub fn ffmpeg_target_for(os: &str, arch: &str) -> Option<(ProcureTarget, ProcureTarget)> {
    // ffmpeg.martin-riedl.de는 세 OS를 같은 경로 규칙으로 제공한다.
    let (plat, ext) = match (os, arch) {
        ("macos", "aarch64") => ("macos/arm64", ""),
        ("macos", "x86_64") => ("macos/amd64", ""),
        ("linux", "aarch64") => ("linux/arm64", ""),
        ("linux", "x86_64") => ("linux/amd64", ""),
        ("windows", "x86_64") => ("windows/amd64", ".exe"),
        _ => return None,
    };
    let make = |bin: &str| ProcureTarget {
        url: format!("https://ffmpeg.martin-riedl.de/redirect/latest/{plat}/release/{bin}.zip"),
        // **비워 둔다.** rolling 빌드라 고정 해시가 없다 (§9.7).
        sha256: String::new(),
        archive_member: Some(format!("{bin}{ext}")),
    };
    Some((make("ffmpeg"), make("ffprobe")))
}

/// 다운로드 후 SHA-256을 검증한다. 불일치면 삭제하고 에러.
///
/// 순서를 지킨다: **임시 파일 → 검증 → `dest`로 이동.** `dest`에 곧바로 쓰면
/// 검증에 실패한 바이트가 잠깐이라도 최종 경로에 실행 가능한 상태로 놓인다.
pub async fn download_verified(target: &ProcureTarget, dest: &std::path::Path) -> Result<()> {
    let want = target.sha256.trim().to_ascii_lowercase();
    if want.is_empty() {
        return Err(SoulError::invalid(
            "해시 미상이라 조달할 수 없습니다. ffmpeg을 직접 설치해 주세요 (§9.7)",
        ));
    }
    if want.len() != 64 || !want.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(SoulError::invalid(format!(
            "SHA-256이 64자리 16진수가 아닙니다: {:?}",
            target.sha256
        )));
    }
    if target.url.trim().is_empty() {
        return Err(SoulError::invalid("조달 URL이 비어 있습니다"));
    }

    // `Path::new("ffmpeg").parent()`는 빈 경로다. 그대로 mkdir 하면 실패한다.
    let parent = match dest.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    tokio::fs::create_dir_all(parent).await?;
    let file_name = dest
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("procured");
    let tmp = parent.join(format!(".{file_name}.part-{}", std::process::id()));

    let res = download_to(&target.url, &tmp, &want).await;
    if res.is_err() {
        // 검증에 실패한 바이트는 남기지 않는다
        let _ = tokio::fs::remove_file(&tmp).await;
        return res;
    }

    tokio::fs::rename(&tmp, dest).await?;

    // 아카이브가 아니라 실행 파일을 직접 받은 경우에만 실행 권한을 준다
    #[cfg(unix)]
    if target.archive_member.is_none() {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = tokio::fs::metadata(dest).await?.permissions();
        perm.set_mode(0o755);
        tokio::fs::set_permissions(dest, perm).await?;
    }
    Ok(())
}

/// 받으면서 해시한다. 파일을 다 받고 다시 읽지 않는다.
async fn download_to(url: &str, tmp: &Path, want_hex: &str) -> Result<()> {
    let client = reqwest::Client::builder()
        // 전체 타임아웃은 두지 않는다 — 70MB를 느린 회선으로 받을 수 있다.
        .connect_timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| SoulError::invalid(format!("HTTP 클라이언트 생성 실패: {e}")))?;

    let mut resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| SoulError::invalid(format!("조달 요청 실패: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(SoulError::invalid(format!("조달 응답 {status}: {url}")));
    }

    let mut file = tokio::fs::File::create(tmp).await?;
    let mut hasher = Sha256::new();
    let mut total: u64 = 0;
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| SoulError::invalid(format!("조달 본문 읽기 실패: {e}")))?
    {
        total += chunk.len() as u64;
        if total > MAX_DOWNLOAD_BYTES {
            return Err(SoulError::invalid(format!(
                "조달 파일이 상한({MAX_DOWNLOAD_BYTES} 바이트)을 넘었습니다: {url}"
            )));
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    file.sync_all().await?;
    drop(file);

    let got = hex::encode(hasher.finalize());
    if got != want_hex {
        return Err(SoulError::invalid(format!(
            "SHA-256 불일치. 기대 {want_hex}, 실제 {got} — 파일을 삭제했습니다"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("soul-net-{tag}-{}-{nanos}", std::process::id()))
    }

    #[test]
    fn platform_table_covers_the_three_desktops() {
        for (os, arch) in [
            ("macos", "aarch64"),
            ("macos", "x86_64"),
            ("linux", "x86_64"),
            ("linux", "aarch64"),
            ("windows", "x86_64"),
        ] {
            let (ffmpeg, ffprobe) = ffmpeg_target_for(os, arch).expect("{os}/{arch}");
            assert!(ffmpeg.url.starts_with("https://"), "{os}/{arch}");
            assert!(ffmpeg.url.contains("ffmpeg"));
            assert!(ffprobe.url.contains("ffprobe"));
            assert_ne!(ffmpeg.url, ffprobe.url);
            // 해시는 비었거나(=조달 거부) 64자리 16진수여야 한다. 그 사이는 없다.
            for t in [&ffmpeg, &ffprobe] {
                assert!(
                    t.sha256.is_empty()
                        || (t.sha256.len() == 64
                            && t.sha256.chars().all(|c| c.is_ascii_hexdigit())),
                    "{os}/{arch}: 해시가 어중간하다 {:?}",
                    t.sha256
                );
            }
        }
        assert!(ffmpeg_target_for("plan9", "x86_64").is_none());
        assert!(ffmpeg_target_for("linux", "riscv64").is_none());
    }

    #[test]
    fn windows_members_have_exe_suffix() {
        let (ffmpeg, ffprobe) = ffmpeg_target_for("windows", "x86_64").unwrap();
        assert_eq!(ffmpeg.archive_member.as_deref(), Some("ffmpeg.exe"));
        assert_eq!(ffprobe.archive_member.as_deref(), Some("ffprobe.exe"));
        let (mac, _) = ffmpeg_target_for("macos", "aarch64").unwrap();
        assert_eq!(mac.archive_member.as_deref(), Some("ffmpeg"));
    }

    /// 검증 없는 실행 파일은 내려받지 않는다.
    #[tokio::test]
    async fn empty_hash_refuses_to_download() {
        let dest = temp_path("nohash");
        let t = ProcureTarget {
            url: "https://example.invalid/ffmpeg.zip".into(),
            sha256: String::new(),
            archive_member: None,
        };
        let err = download_verified(&t, &dest).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("해시 미상"), "메시지: {msg}");
        assert!(msg.contains("직접 설치"), "메시지: {msg}");
        assert!(!dest.exists(), "거부했는데 파일이 생겼다");
    }

    #[tokio::test]
    async fn whitespace_only_hash_is_still_empty() {
        let dest = temp_path("wshash");
        let t = ProcureTarget {
            url: "https://example.invalid/ffmpeg.zip".into(),
            sha256: "   ".into(),
            archive_member: None,
        };
        assert!(download_verified(&t, &dest)
            .await
            .unwrap_err()
            .to_string()
            .contains("해시 미상"));
        assert!(!dest.exists());
    }

    #[tokio::test]
    async fn malformed_hash_is_refused() {
        let dest = temp_path("badhash");
        for bad in ["deadbeef", &"z".repeat(64), &"a".repeat(63)] {
            let t = ProcureTarget {
                url: "https://example.invalid/ffmpeg.zip".into(),
                sha256: bad.to_string(),
                archive_member: None,
            };
            let msg = download_verified(&t, &dest).await.unwrap_err().to_string();
            assert!(msg.contains("64자리 16진수"), "{bad}: {msg}");
        }
        assert!(!dest.exists());
    }

    #[tokio::test]
    async fn current_platform_target_is_not_yet_procurable() {
        // 이 테스트는 "해시를 채우기 전까지 조달이 막혀 있다"는 사실을 고정한다.
        // 해시를 실제로 채우면 이 테스트를 지워라.
        if let Some((ffmpeg, _)) = ffmpeg_target() {
            if ffmpeg.sha256.is_empty() {
                let dest = temp_path("platform");
                let err = download_verified(&ffmpeg, &dest).await.unwrap_err();
                assert!(err.to_string().contains("해시 미상"));
                assert!(!dest.exists());
            }
        }
    }

    #[tokio::test]
    async fn empty_url_is_refused() {
        let dest = temp_path("nourl");
        let t = ProcureTarget {
            url: String::new(),
            sha256: "a".repeat(64),
            archive_member: None,
        };
        assert!(download_verified(&t, &dest)
            .await
            .unwrap_err()
            .to_string()
            .contains("URL이 비어"));
    }
}
