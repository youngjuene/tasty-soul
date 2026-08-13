//! 영상 처리 (§9.6 · §20.5). **앞 30초만 처리한다.**
//!
//! ```bash
//! # 프레임: 30초 구간에서 1fps, 긴 변 1280
//! ffmpeg -i in.mp4 -t 30 \
//!        -vf "fps=1,scale='if(gt(iw,ih),min(1280,iw),-2)':'if(gt(iw,ih),-2,min(1280,ih))'" \
//!        -q:v 3 frame_%03d.jpg
//!
//! # 오디오: 같은 30초 구간
//! ffmpeg -i in.mp4 -t 30 -vn -ac 1 -ar 16000 audio.mp3
//! ```
//!
//! - 30초 미만 영상은 있는 만큼만 뽑는다. **프레임이 3장 미만이면 `quality: partial`** (T65)
//! - **프레임 전체를 한 번의 비전 호출에 배열로 넣는다.** 장당 따로 부르지 않는다
//! - **씬 전환 감지를 쓰지 않는다** — 영상 전체를 디코딩해야 하므로 §20.5를 깬다
//! - 오디오가 없거나 실패하면 프레임만으로 진행, `quality: partial`
//! - 중간 산출물은 `runs/` 트레이스에만 남긴다

use soul_core::error::{Result, SoulError};

pub struct PreparedVideo {
    /// 1fps로 뽑은 프레임 JPEG들. 최대 `video_max_frames`장.
    pub frames: Vec<Vec<u8>>,
    /// 같은 30초 구간의 오디오. 없으면 `None` → `quality: partial`.
    pub audio_mp3: Option<Vec<u8>>,
    pub duration_secs: Option<f64>,
    /// 아카이브용 썸네일 원본 = **추출한 첫 프레임** (§20.4).
    pub first_frame: Option<Vec<u8>>,
}

const DEFAULT_MAX_SECONDS: u32 = 30;
const DEFAULT_FPS: u32 = 1;
const DEFAULT_MAX_FRAMES: u32 = 30;
const DEFAULT_MAX_EDGE: u32 = 1280;

pub fn prepare(
    tools: &crate::ffmpeg::FfmpegTools,
    path: &std::path::Path,
    max_seconds: u32,
    fps: u32,
    max_frames: u32,
    max_edge_px: u32,
) -> Result<PreparedVideo> {
    let max_seconds = nonzero(max_seconds, DEFAULT_MAX_SECONDS);
    let fps = nonzero(fps, DEFAULT_FPS);
    let max_frames = nonzero(max_frames, DEFAULT_MAX_FRAMES);
    let max_edge_px = nonzero(max_edge_px, DEFAULT_MAX_EDGE);

    let input = path
        .to_str()
        .ok_or_else(|| SoulError::invalid("경로가 UTF-8이 아닙니다"))?;
    // 길이를 못 읽어도 처리 자체는 계속한다. `-t`가 상한을 보장한다.
    let duration_secs = crate::ffmpeg::probe(tools, path)
        .ok()
        .and_then(|p| p.duration_secs);

    let dir = tempfile::tempdir()?;
    let pattern = dir.path().join("frame_%04d.jpg");
    let pattern_s = path_str(&pattern)?;
    let secs = max_seconds.to_string();
    let frames_s = max_frames.to_string();
    let filter = frame_filter(fps, max_edge_px);
    crate::ffmpeg::run(
        tools,
        &frame_args(input, &secs, &filter, &frames_s, pattern_s),
    )?;

    // frame_0001.jpg … 파일명 사전순 = 시간순.
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir.path())?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("frame_") && n.ends_with(".jpg"))
        })
        .collect();
    files.sort();
    files.truncate(max_frames as usize);

    let mut frames: Vec<Vec<u8>> = Vec::with_capacity(files.len());
    for f in &files {
        let bytes = std::fs::read(f)?;
        if !bytes.is_empty() {
            frames.push(bytes);
        }
    }
    // 프레임이 3장 미만이어도 여기서 막지 않는다 — quality: partial 판단은 호출자 몫 (T65).
    let first_frame = frames.first().cloned();

    // 오디오는 실패해도 프레임만으로 진행한다 (§9.6).
    let audio_mp3 = extract_audio(tools, dir.path(), input, &secs);

    Ok(PreparedVideo {
        frames,
        audio_mp3,
        duration_secs,
        first_frame,
    })
}

/// §9.6의 필터. `min(E, iw)` 형태라 원본이 작으면 확대하지 않는다 (T63·T66).
/// `-2`는 짝수로 맞춘 자동 계산이라 인코더가 홀수 크기로 실패하지 않는다.
/// **씬 전환 감지를 쓰지 않는다** — 고정 간격 `fps=N`뿐이다 (§20.5).
fn frame_filter(fps: u32, max_edge_px: u32) -> String {
    format!(
        "fps={fps},scale='if(gt(iw,ih),min({e},iw),-2)':'if(gt(iw,ih),-2,min({e},ih))'",
        e = max_edge_px
    )
}

/// **`-t`는 입력 뒤·출력 앞**이다 (T43·T43b). 입력 앞에 두면 탐색이 되어버리고,
/// 출력 뒤로 밀면 30초 이후까지 디코딩한다.
fn frame_args<'a>(
    input: &'a str,
    max_seconds: &'a str,
    filter: &'a str,
    max_frames: &'a str,
    out_pattern: &'a str,
) -> [&'a str; 17] {
    [
        "-y",
        "-nostdin",
        "-loglevel",
        "error",
        "-i",
        input,
        "-t",
        max_seconds,
        "-vf",
        filter,
        "-q:v",
        "3",
        "-frames:v",
        max_frames,
        "-f",
        "image2",
        out_pattern,
    ]
}

/// 같은 30초 구간의 오디오. `-t`의 위치는 프레임 쪽과 같다.
fn audio_args<'a>(input: &'a str, max_seconds: &'a str, out: &'a str) -> [&'a str; 14] {
    [
        "-y",
        "-nostdin",
        "-loglevel",
        "error",
        "-i",
        input,
        "-t",
        max_seconds,
        "-vn",
        "-ac",
        "1",
        "-ar",
        "16000",
        out,
    ]
}

fn extract_audio(
    tools: &crate::ffmpeg::FfmpegTools,
    dir: &std::path::Path,
    input: &str,
    secs: &str,
) -> Option<Vec<u8>> {
    let out = dir.join("audio.mp3");
    let out_s = out.to_str()?;
    crate::ffmpeg::run(tools, &audio_args(input, secs, out_s)).ok()?;
    let bytes = std::fs::read(&out).ok()?;
    if bytes.is_empty() {
        return None;
    }
    Some(bytes)
}

fn nonzero(v: u32, fallback: u32) -> u32 {
    if v == 0 {
        fallback
    } else {
        v
    }
}

fn path_str(p: &std::path::Path) -> Result<&str> {
    p.to_str()
        .ok_or_else(|| SoulError::invalid("임시 경로가 UTF-8이 아닙니다"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T63·T66 — 필터 문자열은 명세 그대로여야 한다. 긴 변만 상한을 받고 짧은 변은
    /// `-2`로 비율을 따라간다.
    #[test]
    fn t63_frame_filter_matches_spec() {
        assert_eq!(
            frame_filter(1, 1280),
            "fps=1,scale='if(gt(iw,ih),min(1280,iw),-2)':'if(gt(iw,ih),-2,min(1280,ih))'"
        );
        assert_eq!(
            frame_filter(2, 640),
            "fps=2,scale='if(gt(iw,ih),min(640,iw),-2)':'if(gt(iw,ih),-2,min(640,ih))'"
        );
    }

    /// T66 — `min(...)` 이므로 원본보다 커지지 않는다. 확대 방지가 필터에 들어 있다.
    #[test]
    fn t66_filter_never_upscales() {
        let f = frame_filter(1, 1280);
        assert!(
            f.contains("min(1280,iw)") && f.contains("min(1280,ih)"),
            "{f}"
        );
    }

    /// **씬 전환 감지를 쓰지 않는다** (§20.5).
    #[test]
    fn no_scene_detection() {
        let f = frame_filter(1, 1280);
        assert!(!f.contains("scene"), "{f}");
        assert!(!f.contains("select"), "{f}");
        assert!(f.starts_with("fps="), "고정 간격이어야 한다: {f}");
    }

    /// T43·T43b — `-t`는 `-i` 뒤, 출력 앞. 이 순서라야 30초 이후를 디코딩하지 않는다.
    #[test]
    fn t43_duration_cap_sits_between_input_and_output() {
        let filter = frame_filter(1, 1280);
        let args = frame_args("in.mp4", "30", &filter, "30", "/tmp/f/frame_%04d.jpg");
        let i = args.iter().position(|a| *a == "-i").unwrap();
        let t = args.iter().position(|a| *a == "-t").unwrap();
        assert!(i < t, "-t 는 입력 뒤에 와야 한다: {args:?}");
        assert_eq!(args[t + 1], "30");
        assert_eq!(args[args.len() - 1], "/tmp/f/frame_%04d.jpg");
        assert!(t < args.len() - 1, "-t 는 출력 앞이다: {args:?}");
        // -ss 로 입력을 건너뛰지 않는다 — 영상은 언제나 앞에서부터다
        assert!(!args.contains(&"-ss"), "{args:?}");
        assert!(args.windows(2).any(|w| w == ["-frames:v", "30"]));
    }

    /// 오디오도 같은 30초 구간이고 모노 16kHz다.
    #[test]
    fn audio_uses_the_same_window() {
        let args = audio_args("in.mp4", "30", "/tmp/a.mp3");
        let i = args.iter().position(|a| *a == "-i").unwrap();
        let t = args.iter().position(|a| *a == "-t").unwrap();
        assert!(i < t, "{args:?}");
        assert!(args.contains(&"-vn"));
        assert!(args.windows(2).any(|w| w == ["-ac", "1"]));
        assert!(args.windows(2).any(|w| w == ["-ar", "16000"]));
        assert_eq!(args[args.len() - 1], "/tmp/a.mp3");
    }

    #[test]
    fn zero_config_falls_back_to_spec_defaults() {
        assert_eq!(nonzero(0, DEFAULT_MAX_FRAMES), 30);
        assert_eq!(nonzero(15, DEFAULT_MAX_FRAMES), 15);
        assert_eq!(
            frame_filter(nonzero(0, DEFAULT_FPS), nonzero(0, DEFAULT_MAX_EDGE)),
            "fps=1,scale='if(gt(iw,ih),min(1280,iw),-2)':'if(gt(iw,ih),-2,min(1280,ih))'"
        );
    }

    /// ffmpeg이 있어야 도는 경로. **네트워크는 쓰지 않는다** — lavfi로 영상을 만들어 쓴다.
    /// `locate`가 `None`이면 건너뛴다.
    #[test]
    fn e2e_only_the_first_30_seconds_are_used() {
        let Some(tools) = crate::ffmpeg::locate(std::path::Path::new("/nonexistent")) else {
            eprintln!("ffmpeg 없음 — 건너뜀");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("clip.mp4");
        // 90초짜리 1920x1080 테스트 영상 + 무음 오디오
        crate::ffmpeg::run(
            &tools,
            &[
                "-y",
                "-nostdin",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=1920x1080:rate=10:duration=90",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=90",
                "-shortest",
                "-pix_fmt",
                "yuv420p",
                src.to_str().unwrap(),
            ],
        )
        .unwrap();

        let v = prepare(&tools, &src, 30, 1, 30, 1280).unwrap();
        assert!(
            v.frames.len() <= 30 && v.frames.len() >= 28,
            "30초분 1fps여야 하는데 {}장",
            v.frames.len()
        );
        assert!(v.first_frame.is_some());
        assert_eq!(v.first_frame.as_ref().unwrap(), &v.frames[0]);
        let img = image::load_from_memory(&v.frames[0]).unwrap();
        assert_eq!(image::GenericImageView::dimensions(&img), (1280, 720));
        assert!(v.audio_mp3.is_some(), "오디오가 있어야 한다");
        assert!(v.duration_secs.unwrap() > 80.0);
    }

    /// 오디오가 없는 영상도 프레임만으로 진행한다 (§9.6).
    #[test]
    fn e2e_silent_video_still_yields_frames() {
        let Some(tools) = crate::ffmpeg::locate(std::path::Path::new("/nonexistent")) else {
            eprintln!("ffmpeg 없음 — 건너뜀");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("silent.mp4");
        crate::ffmpeg::run(
            &tools,
            &[
                "-y",
                "-nostdin",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=320x240:rate=10:duration=2",
                "-pix_fmt",
                "yuv420p",
                src.to_str().unwrap(),
            ],
        )
        .unwrap();

        let v = prepare(&tools, &src, 30, 1, 30, 1280).unwrap();
        assert!(
            v.frames.len() < 3,
            "2초짜리는 3장 미만 → 호출자가 partial (T65)"
        );
        assert!(v.audio_mp3.is_none(), "오디오 스트림이 없다");
        // 확대하지 않는다
        let img = image::load_from_memory(&v.frames[0]).unwrap();
        assert_eq!(image::GenericImageView::dimensions(&img), (320, 240));
    }
}
