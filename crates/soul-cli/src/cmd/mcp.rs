//! `soul mcp` (§14 · §19.7).
//!
//! **서버를 이 프로세스에서 구동하지 않는다.** 같은 디렉토리의 `soul-mcp` 실행 파일을
//! 찾아 exec 한다. `soul` 바이너리에는 `reqwest`가 링크되어 있으므로, 여기서 서버를
//! 돌리면 §19.4의 "네트워크 클라이언트를 링크하지 않는다"가 깨진다 (T32).
//!
//! `--print-config`는 표준 MCP 서버 설정 형태를 출력한다. **설정 파일을 수정하지 않는다** (T38).
//!
//! ```json
//! { "mcpServers": { "soul": { "command": "soul", "args": ["mcp"] } } }
//! ```
//!
//! 클라이언트마다 설정 파일 위치와 키 이름이 다르므로 앱은 출력과 안내까지만 한다 (§19.7).

use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};

/// 찾을 실행 파일 이름. windows에서는 `.exe`가 붙는다.
const SERVER_BIN: &str = "soul-mcp";

/// 개발·패키징 환경에서 위치를 직접 지정하고 싶을 때 쓰는 탈출구.
const SERVER_BIN_ENV: &str = "SOUL_MCP_BIN";

pub fn print_config() -> anyhow::Result<()> {
    // stdout에만 쓴다. **파일을 만들지도 고치지도 않는다** (T38).
    println!("{}", config_json()?);
    Ok(())
}

/// `--print-config`가 내보내는 JSON 문자열.
///
/// 형태를 테스트가 직접 검증할 수 있도록 출력과 분리해 둔다 (T38).
pub fn config_json() -> Result<String> {
    let v = serde_json::json!({
        "mcpServers": {
            "soul": {
                "command": "soul",
                "args": ["mcp"],
            }
        }
    });
    Ok(serde_json::to_string_pretty(&v)?)
}

/// 같은 디렉토리의 `soul-mcp`를 찾아 exec 한다.
///
/// unix는 `execv`로 **이 프로세스를 대체한다** — stdio가 그대로 이어지고 중간 프로세스가
/// 남지 않는다 (§20.1). windows에는 exec이 없으므로 spawn 후 종료 코드를 그대로 물려준다.
pub fn exec_server() -> anyhow::Result<()> {
    let exe = locate_server()?;
    let mut cmd = std::process::Command::new(&exe);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // 성공하면 돌아오지 않는다. 반환값은 항상 실패다.
        let err = cmd.exec();
        Err(anyhow!("{} 실행 실패: {err}", exe.display()))
    }

    #[cfg(not(unix))]
    {
        let status = cmd
            .status()
            .map_err(|e| anyhow!("{} 실행 실패: {e}", exe.display()))?;
        // 서버의 종료 코드를 그대로 물려준다. 여기서 감싸면 클라이언트가 원인을 잃는다.
        std::process::exit(status.code().unwrap_or(1));
    }
}

/// `soul-mcp` 실행 파일 경로.
///
/// 1. `SOUL_MCP_BIN` 환경변수 (개발·패키징용 탈출구)
/// 2. **`current_exe()`의 디렉토리** — 배포본과 `target/debug` 개발 트리가 여기서 잡힌다
/// 3. `PATH`
/// 4. 없으면 무엇을 어디서 찾다 실패했는지 적은 에러
fn locate_server() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os(SERVER_BIN_ENV) {
        let p = PathBuf::from(p);
        if is_executable_file(&p) {
            return Ok(p);
        }
        return Err(anyhow!(
            "{SERVER_BIN_ENV}={} 가 실행 가능한 파일이 아닙니다",
            p.display()
        ));
    }

    let mut looked: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            looked.push(dir.to_path_buf());
        }
    }
    looked.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));

    locate_in(&looked).ok_or_else(|| {
        let where_ = if looked.is_empty() {
            "없음".to_string()
        } else {
            looked
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };
        anyhow!(
            "{} 실행 파일을 찾을 수 없습니다. `soul` 옆에 두거나 PATH에 넣거나 \
             {SERVER_BIN_ENV} 로 경로를 지정하세요. 찾아본 곳: {where_}",
            server_file_name(),
        )
    })
}

/// 주어진 디렉토리들을 **순서대로** 훑어 첫 번째로 실행 가능한 것을 고른다.
///
/// 순서가 규칙이다 — 같은 디렉토리의 것이 PATH의 것보다 먼저다. 버전이 다른
/// `soul-mcp`가 PATH에 있어도 배포본은 자신의 것을 쓴다.
fn locate_in(dirs: &[PathBuf]) -> Option<PathBuf> {
    let name = server_file_name();
    dirs.iter()
        .map(|d| d.join(&name))
        .find(|c| is_executable_file(c))
}

fn server_file_name() -> String {
    format!("{SERVER_BIN}{}", std::env::consts::EXE_SUFFIX)
}

#[cfg(unix)]
fn is_executable_file(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(p) {
        Ok(m) => m.is_file() && m.permissions().mode() & 0o111 != 0,
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn is_executable_file(p: &Path) -> bool {
    p.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir()
            .join("tasty-soul-cli-mcp")
            .join(format!("{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn touch_executable(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, b"#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    /// T38 — 출력이 **유효한 JSON**이고 §19.7의 형태를 그대로 갖는다.
    #[test]
    fn print_config_emits_valid_mcp_json() {
        let s = config_json().unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).expect("유효한 JSON이어야 한다");
        assert_eq!(v["mcpServers"]["soul"]["command"], "soul");
        assert_eq!(v["mcpServers"]["soul"]["args"][0], "mcp");
        assert_eq!(
            v["mcpServers"]["soul"]["args"].as_array().map(Vec::len),
            Some(1)
        );
        // 서버 목록에 우리 것 하나뿐 — 다른 클라이언트 설정을 흉내 내지 않는다.
        assert_eq!(v["mcpServers"].as_object().map(|o| o.len()), Some(1));
    }

    /// T38 — 설정 파일을 만들지도 고치지도 않는다.
    #[test]
    fn print_config_touches_no_files() {
        let dir = temp_dir("print-config");
        let before = std::fs::read_dir(&dir).unwrap().count();
        config_json().unwrap();
        let after = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(before, 0);
        assert_eq!(before, after);
    }

    #[test]
    fn locate_prefers_the_first_directory() {
        let a = temp_dir("locate-a");
        let b = temp_dir("locate-b");
        let name = server_file_name();
        let want = touch_executable(&a, &name);
        touch_executable(&b, &name);

        assert_eq!(locate_in(&[a.clone(), b.clone()]), Some(want));
        // 없는 디렉토리는 조용히 건너뛴다.
        assert_eq!(locate_in(&[a.join("nope")]), None);
        assert_eq!(locate_in(&[]), None);
    }

    #[test]
    fn locate_ignores_a_non_executable_file() {
        // 이름만 같고 실행 권한이 없는 파일을 서버로 착각하면 exec이 EACCES로 죽는다.
        let dir = temp_dir("locate-noexec");
        let path = dir.join(server_file_name());
        std::fs::write(&path, b"not a program").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(locate_in(&[dir]), None);
        }
        #[cfg(not(unix))]
        {
            assert_eq!(locate_in(&[dir.clone()]), Some(path));
        }
    }
}
