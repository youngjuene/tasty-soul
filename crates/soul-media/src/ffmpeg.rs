//! ffmpeg/ffprobe 실행 (§9.7).
//!
//! 1. PATH에서 `ffmpeg`·`ffprobe`를 먼저 찾는다
//! 2. 없으면 최초 오디오/영상 투입 시 사용자에게 알리고 정적 빌드를 `<root>/bin/`에 다운로드
//!    (다운로드 자체는 `soul-net::procure`가 한다)
//! 3. SHA-256 검증 실패 시 삭제 후 중단
//! 4. **실행 파일에 번들하지 않는다** (70MB) — §20.8
//!
//! 실패 시 **명령어와 stderr를 트레이스에 기록한다** (§15).

use soul_core::error::{Result, SoulError};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Debug)]
pub struct FfmpegTools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

/// 실행 파일 이름. 윈도우는 `.exe`가 붙는다.
const FFMPEG_EXE: &str = if cfg!(windows) {
    "ffmpeg.exe"
} else {
    "ffmpeg"
};
const FFPROBE_EXE: &str = if cfg!(windows) {
    "ffprobe.exe"
} else {
    "ffprobe"
};

/// 에러 메시지에 담는 stderr 길이 상한. 뒤쪽이 원인에 가까우므로 **꼬리**를 남긴다.
const STDERR_TAIL_LIMIT: usize = 4000;

/// PATH → `<root>/bin/` 순으로 찾는다. 없으면 `None`.
///
/// `which` 같은 외부 명령에 의존하지 않고 PATH 환경변수를 직접 훑는다.
/// 두 실행 파일을 **각각** 찾으므로 PATH의 ffmpeg과 조달된 ffprobe가 섞여도 된다.
pub fn locate(bin_dir: &Path) -> Option<FfmpegTools> {
    let ffmpeg = find_tool(bin_dir, FFMPEG_EXE)?;
    let ffprobe = find_tool(bin_dir, FFPROBE_EXE)?;
    Some(FfmpegTools { ffmpeg, ffprobe })
}

/// PATH의 디렉토리를 앞에서부터, 그다음 `bin_dir`을 본다.
fn find_tool(bin_dir: &Path, exe: &str) -> Option<PathBuf> {
    let path_dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    find_exe(path_dirs.iter().map(PathBuf::as_path), exe)
        .or_else(|| find_exe(std::iter::once(bin_dir), exe))
}

/// 주어진 디렉토리들에서 실행 가능한 `exe`를 찾는다. 순서가 곧 우선순위다.
fn find_exe<'a>(dirs: impl Iterator<Item = &'a Path>, exe: &str) -> Option<PathBuf> {
    for dir in dirs {
        // PATH에 빈 항목이 섞이면 현재 디렉토리를 뜻하게 되어 위험하다. 건너뛴다.
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(exe);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(p) {
        // 디렉토리 이름이 `ffmpeg`인 경우를 걸러내려면 is_file 검사가 필요하다.
        Ok(md) => md.is_file() && md.permissions().mode() & 0o111 != 0,
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    std::fs::metadata(p).map(|md| md.is_file()).unwrap_or(false)
}

#[derive(Clone, Debug, Default)]
pub struct ProbeInfo {
    pub duration_secs: Option<f64>,
    pub has_video: bool,
    pub has_audio: bool,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// GIF·애니메이션 WebP 판정용 (§9.1 단계 4 예외).
    pub format_name: String,
    pub nb_frames: Option<u64>,
}

pub fn probe(tools: &FfmpegTools, path: &Path) -> Result<ProbeInfo> {
    let path_arg = path.to_string_lossy().into_owned();
    let args: Vec<&str> = vec![
        "-v",
        "error",
        "-show_streams",
        "-show_format",
        "-of",
        "json",
        &path_arg,
    ];

    let out = Command::new(&tools.ffprobe)
        .args(&args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| {
            SoulError::invalid(format!(
                "ffprobe를 실행하지 못했습니다: {e}\n  명령: {}",
                cmdline(&tools.ffprobe, &args)
            ))
        })?;

    if !out.status.success() {
        // §15 — 명령어와 stderr를 그대로 담는다.
        return Err(SoulError::invalid(format!(
            "ffprobe 실패 ({}): {}\n--- stderr ---\n{}",
            exit_desc(&out.status),
            cmdline(&tools.ffprobe, &args),
            tail(&String::from_utf8_lossy(&out.stderr), STDERR_TAIL_LIMIT)
        )));
    }

    let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| {
        SoulError::invalid(format!(
            "ffprobe 출력을 JSON으로 읽지 못했습니다: {e}\n  명령: {}",
            cmdline(&tools.ffprobe, &args)
        ))
    })?;

    Ok(parse_probe_json(&v))
}

/// ffprobe JSON → `ProbeInfo`. 순수 함수라 테스트로 고정할 수 있다.
fn parse_probe_json(v: &serde_json::Value) -> ProbeInfo {
    let mut info = ProbeInfo {
        format_name: v["format"]["format_name"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        ..ProbeInfo::default()
    };

    info.duration_secs = read_duration(&v["format"]["duration"]);

    let mut stream_duration_max: Option<f64> = None;
    for st in v["streams"].as_array().into_iter().flatten() {
        if let Some(d) = read_duration(&st["duration"]) {
            stream_duration_max = Some(stream_duration_max.map_or(d, |m: f64| m.max(d)));
        }
        match st["codec_type"].as_str().unwrap_or_default() {
            "video" => {
                // mp3 앨범 아트는 비디오 스트림으로 보이지만 영상이 아니다.
                // 이걸 세면 커버 아트가 붙은 오디오가 `video`로 오판된다.
                if st["disposition"]["attached_pic"].as_i64().unwrap_or(0) == 1 {
                    continue;
                }
                if !info.has_video {
                    info.width = st["width"].as_u64().map(|x| x as u32);
                    info.height = st["height"].as_u64().map(|x| x as u32);
                    info.nb_frames = read_u64(&st["nb_frames"]);
                }
                info.has_video = true;
            }
            "audio" => info.has_audio = true,
            _ => {}
        }
    }

    // 컨테이너가 길이를 안 적어두는 경우(mkv 일부)를 위한 폴백.
    if info.duration_secs.is_none() {
        info.duration_secs = stream_duration_max;
    }
    info
}

/// ffprobe는 수치를 문자열로 준다. `"N/A"`도 흔하다.
fn read_duration(v: &serde_json::Value) -> Option<f64> {
    let d = match v {
        serde_json::Value::String(s) => s.parse::<f64>().ok()?,
        serde_json::Value::Number(n) => n.as_f64()?,
        _ => return None,
    };
    (d.is_finite() && d >= 0.0).then_some(d)
}

fn read_u64(v: &serde_json::Value) -> Option<u64> {
    match v {
        serde_json::Value::String(s) => s.parse::<u64>().ok(),
        serde_json::Value::Number(n) => n.as_u64(),
        _ => None,
    }
}

/// 인자를 그대로 넘겨 실행한다. 실패 시 stderr를 에러 메시지에 담는다.
///
/// 인자를 **가공하지 않는다** — 호출자가 준 것이 그대로 나간다.
/// 다만 stdin은 막는다. ffmpeg이 덮어쓰기 여부를 물으며 멈추는 일을 방지한다.
///
/// 반환값은 stdout이며, ffmpeg처럼 stdout이 비는 경우에는 로그(stderr)를 돌려준다.
pub fn run(tools: &FfmpegTools, args: &[&str]) -> Result<String> {
    let out = Command::new(&tools.ffmpeg)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| {
            SoulError::invalid(format!(
                "ffmpeg을 실행하지 못했습니다: {e}\n  명령: {}",
                cmdline(&tools.ffmpeg, args)
            ))
        })?;

    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() {
        // §15 — 무엇을 실행했고 무엇이 잘못됐는지가 둘 다 있어야 한다.
        return Err(SoulError::invalid(format!(
            "ffmpeg 실패 ({}): {}\n--- stderr ---\n{}",
            exit_desc(&out.status),
            cmdline(&tools.ffmpeg, args),
            tail(&stderr, STDERR_TAIL_LIMIT)
        )));
    }

    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    Ok(if stdout.is_empty() { stderr } else { stdout })
}

/// 사람이 그대로 붙여 재현할 수 있는 형태로 명령줄을 만든다.
fn cmdline<S: AsRef<str>>(exe: &Path, args: &[S]) -> String {
    let mut s = quote(&exe.to_string_lossy());
    for a in args {
        s.push(' ');
        s.push_str(&quote(a.as_ref()));
    }
    s
}

fn quote(s: &str) -> String {
    if !s.is_empty() && !s.contains([' ', '\t', '"', '\'', '\n']) {
        s.to_string()
    } else {
        format!("\"{}\"", s.replace('"', "\\\""))
    }
}

fn exit_desc(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(c) => format!("exit {c}"),
        None => "시그널로 종료".to_string(),
    }
}

/// 뒤에서부터 `max` 바이트 근처까지 남긴다. 문자 경계를 깨지 않는다.
fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut cut = s.len() - max;
    while cut < s.len() && !s.is_char_boundary(cut) {
        cut += 1;
    }
    format!("…(앞부분 생략)\n{}", &s[cut..])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ffmpeg이 없는 CI에서는 조용히 건너뛴다.
    fn tools_or_skip() -> Option<FfmpegTools> {
        locate(Path::new("/nonexistent-bin-dir"))
    }

    #[test]
    fn find_exe_는_실행_권한이_있는_파일만_고른다() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ffmpeg");
        std::fs::write(&p, b"#!/bin/sh\n").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // 권한 없음 → 못 찾는다
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(find_exe(std::iter::once(dir.path()), "ffmpeg").is_none());
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        assert_eq!(find_exe(std::iter::once(dir.path()), "ffmpeg"), Some(p));
    }

    #[test]
    fn find_exe_는_같은_이름의_디렉토리를_실행파일로_보지_않는다() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("ffprobe")).unwrap();
        assert!(find_exe(std::iter::once(dir.path()), "ffprobe").is_none());
    }

    #[test]
    fn find_exe_는_빈_path_항목을_건너뛴다() {
        let empty = Path::new("");
        assert!(find_exe(std::iter::once(empty), "ffmpeg").is_none());
    }

    #[test]
    fn find_exe_는_앞_디렉토리를_우선한다() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        for d in [a.path(), b.path()] {
            let p = d.join("ffmpeg");
            std::fs::write(&p, b"x").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        let found = find_exe([a.path(), b.path()].into_iter(), "ffmpeg").unwrap();
        assert_eq!(found, a.path().join("ffmpeg"));
    }

    #[test]
    fn find_tool_은_path에_없으면_bin_dir로_폴백한다() {
        // PATH에 절대 없을 이름을 써서 폴백 경로만 결정적으로 시험한다.
        let dir = tempfile::tempdir().unwrap();
        let name = "soul-test-not-in-path";
        assert!(find_tool(dir.path(), name).is_none(), "아직 없어야 한다");

        let p = dir.path().join(name);
        std::fs::write(&p, b"x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(find_tool(dir.path(), name), Some(p));
    }

    #[test]
    fn locate_는_둘_다_있어야_some을_준다() {
        let dir = tempfile::tempdir().unwrap();
        match locate(dir.path()) {
            // 시스템 PATH에 둘 다 있는 경우
            Some(t) => {
                assert!(is_executable(&t.ffmpeg));
                assert!(is_executable(&t.ffprobe));
            }
            // 하나라도 없으면 None
            None => assert!(
                find_tool(dir.path(), FFMPEG_EXE).is_none()
                    || find_tool(dir.path(), FFPROBE_EXE).is_none()
            ),
        }
    }

    #[test]
    fn parse_probe_json_은_앨범아트를_비디오로_세지_않는다() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"streams":[
                 {"codec_type":"video","width":300,"height":300,"disposition":{"attached_pic":1}},
                 {"codec_type":"audio"}],
                "format":{"format_name":"mp3","duration":"12.5"}}"#,
        )
        .unwrap();
        let info = parse_probe_json(&v);
        assert!(!info.has_video, "attached_pic 은 영상이 아니다");
        assert!(info.has_audio);
        assert_eq!(info.duration_secs, Some(12.5));
        assert_eq!(info.format_name, "mp3");
    }

    #[test]
    fn parse_probe_json_은_문자열_수치와_스트림_길이_폴백을_처리한다() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"streams":[
                 {"codec_type":"video","width":320,"height":240,"nb_frames":"20","duration":"2.0"}],
                "format":{"format_name":"matroska,webm","duration":"N/A"}}"#,
        )
        .unwrap();
        let info = parse_probe_json(&v);
        assert!(info.has_video);
        assert_eq!(info.width, Some(320));
        assert_eq!(info.height, Some(240));
        assert_eq!(info.nb_frames, Some(20));
        assert_eq!(
            info.duration_secs,
            Some(2.0),
            "format 길이가 없으면 스트림에서 가져온다"
        );
    }

    #[test]
    fn cmdline_은_공백이_있는_인자를_따옴표로_감싼다() {
        let s = cmdline(Path::new("/usr/bin/ffmpeg"), &["-i", "/a b/c.mp4"]);
        assert_eq!(s, "/usr/bin/ffmpeg -i \"/a b/c.mp4\"");
    }

    #[test]
    fn tail_은_문자_경계를_깨지_않는다() {
        let s = "가".repeat(3000);
        let t = tail(&s, 100);
        assert!(t.len() < s.len());
        assert!(t.ends_with('가'));
    }

    #[test]
    fn probe_는_실제_영상의_해상도와_길이를_읽는다() {
        let Some(tools) = tools_or_skip() else {
            return; // ffmpeg 없음 — 건너뜀
        };
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("v.mp4");
        run(
            &tools,
            &[
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=2:size=320x240:rate=10",
                out.to_str().unwrap(),
            ],
        )
        .unwrap();

        let info = probe(&tools, &out).unwrap();
        assert!(info.has_video);
        assert!(!info.has_audio);
        assert_eq!(info.width, Some(320));
        assert_eq!(info.height, Some(240));
        assert!(info.format_name.contains("mp4"));
        let d = info.duration_secs.unwrap();
        assert!((d - 2.0).abs() < 0.5, "duration={d}");
    }

    #[test]
    fn run_실패시_명령어와_stderr가_에러에_담긴다() {
        let Some(tools) = tools_or_skip() else {
            return;
        };
        let err = run(&tools, &["-i", "/nonexistent/definitely-missing.mp4"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("ffmpeg 실패"), "{err}");
        assert!(err.contains("/nonexistent/definitely-missing.mp4"), "{err}");
        assert!(err.contains("stderr"), "{err}");
    }

    #[test]
    fn probe_는_실행파일이_없으면_명령어를_담은_에러를_낸다() {
        let tools = FfmpegTools {
            ffmpeg: PathBuf::from("/nonexistent/ffmpeg"),
            ffprobe: PathBuf::from("/nonexistent/ffprobe"),
        };
        let err = probe(&tools, Path::new("/tmp/x.mp4"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("/nonexistent/ffprobe"), "{err}");
    }
}
