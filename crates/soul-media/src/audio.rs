//! 오디오 샘플링 (§9.5 · §20.5).
//!
//! **총 30초를 넘기지 않는다** (`audio_cap_seconds`). 받은 클립 길이 `L`에 따라:
//!
//! | 조건 | 샘플링 |
//! |---|---|
//! | `L ≤ 30s` | 전체 (T62) |
//! | `L > 30s` | `[0,10) ∪ [L/2−5, L/2+5) ∪ [L−10, L)` 세 구간을 concat (T61) |
//!
//! 세 구간으로 나누는 이유: 곡의 도입·본체·마무리가 서로 크게 다를 수 있고,
//! 한 곳만 보면 전체 성격을 놓친다.
//!
//! ```bash
//! ffmpeg -ss 0        -t 10 -i clip.m4a -ac 1 -ar 16000 a0.mp3
//! ffmpeg -ss <L/2-5>  -t 10 -i clip.m4a -ac 1 -ar 16000 a1.mp3
//! ffmpeg -ss <L-10>   -t 10 -i clip.m4a -ac 1 -ar 16000 a2.mp3
//! ffmpeg -f concat -safe 0 -i list.txt -c copy out.mp3
//! ```
//!
//! `-ss`를 **입력 앞에** 두어 전체 트랜스코딩을 피한다 (§20.5).

use soul_core::error::{Result, SoulError};

pub struct PreparedAudio {
    pub mp3: Vec<u8>,
    pub seconds: f64,
    /// 세 구간을 이어붙였는가. 프롬프트(§10.2)에 이 사실을 명시해야 한다.
    pub concatenated: bool,
}

/// 기본 상한. `cap_seconds`가 0이면 이 값을 쓴다.
const DEFAULT_CAP: u32 = 30;

/// §9.5 샘플링 계획. ffmpeg 없이도 검증할 수 있도록 분리해 둔다.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Plan {
    /// 전체를 그대로 (T62). 길이를 모르면 상한까지만.
    Whole { seconds: f64 },
    /// 세 구간 concat (T61).
    Thirds { starts: [f64; 3], seg: f64 },
}

/// `L`과 상한으로 계획을 세운다.
fn plan(duration: Option<f64>, cap_seconds: u32) -> Plan {
    let cap = f64::from(if cap_seconds == 0 {
        DEFAULT_CAP
    } else {
        cap_seconds
    });
    match duration.filter(|d| d.is_finite() && *d > 0.0) {
        // L > 30s — [0,10) ∪ [L/2−5, L/2+5) ∪ [L−10, L)
        Some(l) if l > cap => {
            let seg = cap / 3.0;
            let starts = [0.0, (l / 2.0 - seg / 2.0).max(0.0), (l - seg).max(0.0)];
            Plan::Thirds { starts, seg }
        }
        // L ≤ 30s — 전체
        Some(l) => Plan::Whole { seconds: l },
        // 길이를 알 수 없다 — 상한까지만 자른다
        None => Plan::Whole { seconds: cap },
    }
}

pub fn prepare(
    tools: &crate::ffmpeg::FfmpegTools,
    path: &std::path::Path,
    cap_seconds: u32,
) -> Result<PreparedAudio> {
    let input = path
        .to_str()
        .ok_or_else(|| SoulError::invalid("경로가 UTF-8이 아닙니다"))?;
    let duration = crate::ffmpeg::probe(tools, path)
        .ok()
        .and_then(|p| p.duration_secs);
    let dir = tempfile::tempdir()?;

    match plan(duration, cap_seconds) {
        Plan::Whole { seconds } => {
            let out = dir.path().join("out.mp3");
            let out_s = path_str(&out)?;
            let t = fmt_secs(seconds);
            crate::ffmpeg::run(tools, &whole_args(input, &t, out_s))?;
            Ok(PreparedAudio {
                mp3: read_nonempty(&out)?,
                seconds,
                concatenated: false,
            })
        }
        Plan::Thirds { starts, seg } => {
            let seg_s = fmt_secs(seg);
            let mut parts: Vec<std::path::PathBuf> = Vec::with_capacity(3);
            for (i, start) in starts.iter().enumerate() {
                let part = dir.path().join(format!("a{i}.mp3"));
                let part_s = path_str(&part)?;
                let ss = fmt_secs(*start);
                crate::ffmpeg::run(tools, &segment_args(input, &ss, &seg_s, part_s))?;
                read_nonempty(&part)?; // 빈 구간이면 여기서 걸린다
                parts.push(part);
            }

            // concat demuxer 목록. 경로는 작은따옴표로 감싸고 내부 따옴표를 이스케이프한다.
            let list = dir.path().join("list.txt");
            let mut body = String::new();
            for p in &parts {
                body.push_str(&format!("file '{}'\n", path_str(p)?.replace('\'', "'\\''")));
            }
            std::fs::write(&list, body)?;

            let out = dir.path().join("out.mp3");
            crate::ffmpeg::run(tools, &concat_args(path_str(&list)?, path_str(&out)?))?;
            Ok(PreparedAudio {
                mp3: read_nonempty(&out)?,
                seconds: seg * 3.0,
                concatenated: true,
            })
        }
    }
}

/// 전체를 상한까지 트랜스코딩한다. `-t`는 입력 뒤에 둔다.
fn whole_args<'a>(input: &'a str, cap: &'a str, out: &'a str) -> [&'a str; 13] {
    [
        "-y",
        "-nostdin",
        "-loglevel",
        "error",
        "-i",
        input,
        "-t",
        cap,
        "-ac",
        "1",
        "-ar",
        "16000",
        out,
    ]
}

/// 한 구간. **`-ss`를 `-i` 앞에** 두어야 앞부분을 통째로 디코딩하지 않는다 (§20.5).
fn segment_args<'a>(input: &'a str, ss: &'a str, seg: &'a str, out: &'a str) -> [&'a str; 15] {
    [
        "-y",
        "-nostdin",
        "-loglevel",
        "error",
        "-ss",
        ss,
        "-t",
        seg,
        "-i",
        input,
        "-ac",
        "1",
        "-ar",
        "16000",
        out,
    ]
}

/// 이어붙이기는 재인코딩 없이 `-c copy`로 한다.
fn concat_args<'a>(list: &'a str, out: &'a str) -> [&'a str; 13] {
    [
        "-y",
        "-nostdin",
        "-loglevel",
        "error",
        "-f",
        "concat",
        "-safe",
        "0",
        "-i",
        list,
        "-c",
        "copy",
        out,
    ]
}

fn fmt_secs(v: f64) -> String {
    format!("{:.3}", v.max(0.0))
}

fn path_str(p: &std::path::Path) -> Result<&str> {
    p.to_str()
        .ok_or_else(|| SoulError::invalid("임시 경로가 UTF-8이 아닙니다"))
}

fn read_nonempty(p: &std::path::Path) -> Result<Vec<u8>> {
    let bytes = std::fs::read(p)?;
    if bytes.is_empty() {
        return Err(SoulError::invalid(format!(
            "ffmpeg이 빈 오디오를 만들었습니다: {}",
            p.display()
        )));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T62 — 30초 이하면 전체를 쓴다. 이어붙이지 않는다.
    #[test]
    fn t62_short_clip_is_whole() {
        assert_eq!(plan(Some(12.0), 30), Plan::Whole { seconds: 12.0 });
        assert_eq!(plan(Some(30.0), 30), Plan::Whole { seconds: 30.0 });
    }

    /// T61 — 30초를 넘으면 [0,10) ∪ [L/2−5, L/2+5) ∪ [L−10, L).
    #[test]
    fn t61_long_clip_uses_three_windows() {
        let Plan::Thirds { starts, seg } = plan(Some(100.0), 30) else {
            panic!("세 구간이어야 한다");
        };
        assert_eq!(seg, 10.0);
        assert_eq!(starts, [0.0, 45.0, 90.0]);
        // 명세 그대로: 두 번째는 L/2−5, 세 번째는 L−10
        assert_eq!(starts[1], 100.0 / 2.0 - 5.0);
        assert_eq!(starts[2], 100.0 - 10.0);
    }

    /// 상한을 넘기는 순간부터 세 구간이 된다. 구간이 겹쳐도 명세대로 둔다.
    #[test]
    fn just_over_cap_still_uses_three_windows() {
        let Plan::Thirds { starts, seg } = plan(Some(31.0), 30) else {
            panic!("세 구간이어야 한다");
        };
        assert_eq!(seg, 10.0);
        assert_eq!(starts, [0.0, 10.5, 21.0]);
        assert!(starts.iter().all(|s| *s >= 0.0));
    }

    /// 길이를 모르면 상한까지만 자른다. 총합은 언제나 상한 이하다.
    #[test]
    fn unknown_duration_is_capped() {
        assert_eq!(plan(None, 30), Plan::Whole { seconds: 30.0 });
        assert_eq!(plan(Some(f64::NAN), 30), Plan::Whole { seconds: 30.0 });
        assert_eq!(plan(Some(0.0), 30), Plan::Whole { seconds: 30.0 });
        // cap_seconds=0 이면 기본 30초 상한을 쓴다
        assert_eq!(
            plan(Some(1e9), 0),
            Plan::Thirds {
                starts: [0.0, 499_999_995.0, 999_999_990.0],
                seg: 10.0
            }
        );
    }

    /// §20.5 — `-ss`는 `-i` 앞에 있어야 한다. 뒤에 두면 앞부분을 전부 디코딩한다.
    #[test]
    fn seek_comes_before_input() {
        let args = segment_args("in.m4a", "45.000", "10.000", "a1.mp3");
        let ss = args.iter().position(|a| *a == "-ss").unwrap();
        let i = args.iter().position(|a| *a == "-i").unwrap();
        let t = args.iter().position(|a| *a == "-t").unwrap();
        assert!(ss < i, "{args:?}");
        assert!(t < i, "-t 도 입력 앞에 둔다: {args:?}");
        assert_eq!(args[args.len() - 1], "a1.mp3");
        assert!(args.contains(&"-ac") && args.contains(&"1"));
        assert!(args.contains(&"-ar") && args.contains(&"16000"));
    }

    /// 전체 경로에서는 `-t`가 입력 뒤에 온다 (전체를 열되 상한까지만 쓴다).
    #[test]
    fn whole_path_caps_after_input() {
        let args = whole_args("in.m4a", "30.000", "out.mp3");
        let i = args.iter().position(|a| *a == "-i").unwrap();
        let t = args.iter().position(|a| *a == "-t").unwrap();
        assert!(i < t, "{args:?}");
        assert_eq!(args[args.len() - 1], "out.mp3");
    }

    #[test]
    fn concat_uses_demuxer_without_reencoding() {
        let args = concat_args("list.txt", "out.mp3");
        assert!(args.windows(2).any(|w| w == ["-f", "concat"]));
        assert!(args.windows(2).any(|w| w == ["-safe", "0"]));
        assert!(args.windows(2).any(|w| w == ["-c", "copy"]));
        assert_eq!(args[args.len() - 1], "out.mp3");
    }

    #[test]
    fn seconds_are_formatted_for_ffmpeg() {
        assert_eq!(fmt_secs(45.0), "45.000");
        assert_eq!(fmt_secs(-1.0), "0.000");
        assert_eq!(fmt_secs(10.0 / 3.0), "3.333");
    }

    /// ffmpeg이 있어야 도는 경로. **네트워크는 쓰지 않는다** — lavfi로 소리를 만들어 쓴다.
    /// `locate`가 `None`이면 건너뛴다.
    #[test]
    fn e2e_three_windows_are_concatenated() {
        let Some(tools) = crate::ffmpeg::locate(std::path::Path::new("/nonexistent")) else {
            eprintln!("ffmpeg 없음 — 건너뜀");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("tone.wav");
        crate::ffmpeg::run(
            &tools,
            &[
                "-y",
                "-nostdin",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=60",
                src.to_str().unwrap(),
            ],
        )
        .unwrap();

        let long = prepare(&tools, &src, 30).unwrap();
        assert!(long.concatenated, "60초짜리는 이어붙여야 한다");
        assert!((long.seconds - 30.0).abs() < 0.01);
        assert!(!long.mp3.is_empty());

        let short_src = dir.path().join("short.wav");
        crate::ffmpeg::run(
            &tools,
            &[
                "-y",
                "-nostdin",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=12",
                short_src.to_str().unwrap(),
            ],
        )
        .unwrap();
        let short = prepare(&tools, &short_src, 30).unwrap();
        assert!(!short.concatenated, "12초짜리는 전체를 쓴다");
    }
}
