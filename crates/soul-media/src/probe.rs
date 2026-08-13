//! kind 판별 (§9.1) — **확장자를 신뢰하지 않는다.**
//!
//! | 순서 | 조건 | 결과 |
//! |---|---|---|
//! | 1 | 클립보드가 URL이고 YouTube 도메인 | §9.3으로. `kind`는 §9.3이 정한다 |
//! | 2 | 클립보드가 URL이고 YouTube 아님 | **거부.** "YouTube 링크만 받습니다" |
//! | 3 | 클립보드가 URL 아님 | `text` |
//! | 4 | 파일이고 **magic bytes**가 영상 컨테이너 | `video` |
//! | 5 | 파일이고 magic bytes가 이미지 | `image` |
//! | 6 | 파일이고 magic bytes가 오디오 | **거부.** "오디오는 YouTube 링크로 넣어주세요" |
//! | 7 | 그 외 파일 | 거부. 사유 표시 |
//!
//! `.webm`은 영상일 수도 애니메이션일 수도 있고, 확장자가 없거나 틀린 파일도 흔하다.
//! 컨테이너를 실제로 열어 비디오 스트림이 있는지 확인한다 (`ffprobe -show_streams`) — T11f.
//!
//! **애니메이션 GIF·WebP는 비디오 스트림이 있더라도 `image`로 처리한다** (§9.4 첫 프레임 규칙).
//! 단계 4의 판정에서 GIF·애니메이션 WebP는 제외한다 (T11h·T25c).

use soul_core::obs::Kind;
use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub enum DetectedKind {
    Accepted {
        kind: Kind,
        mime: String,
    },
    /// 거부 사유는 사용자에게 그대로 보인다.
    Rejected {
        reason: String,
    },
    /// YouTube URL. `kind`는 §9.3의 해석기가 정한다.
    YouTube {
        video_id: String,
        canonical_url: String,
    },
}

/// §9.1 단계 6 고정 문구.
const AUDIO_REJECT: &str = "오디오 파일은 받지 않습니다. 오디오는 YouTube 링크로 넣어주세요.";
/// §9.1 단계 2 고정 문구.
const URL_REJECT: &str = "YouTube 링크만 받습니다.";

/// magic bytes 판정에 읽는 앞부분. mkv 매처가 256바이트를 요구하므로 넉넉히 잡는다.
const HEAD_BYTES: usize = 8192;

// ---------------------------------------------------------------------------
// 클립보드 (단계 1~3)
// ---------------------------------------------------------------------------

/// 클립보드 문자열 판별 (단계 1~3).
pub fn detect_clipboard(text: &str) -> DetectedKind {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return rejected("빈 입력입니다.");
    }

    if !looks_like_url(trimmed) {
        // 단계 3
        return accepted(Kind::Text, "text/plain");
    }

    // 단계 1
    if let Some(id) = youtube_video_id(trimmed) {
        return DetectedKind::YouTube {
            canonical_url: youtube_canonical_url(&id),
            video_id: id,
        };
    }

    // 단계 2. YouTube 도메인이지만 영상 id가 없는 경우는 사유를 따로 알린다 (§9.3 단계 1 실패).
    if is_youtube_url(trimmed) {
        return rejected(format!(
            "{URL_REJECT} 이 링크에서 영상 id를 찾지 못했습니다 (채널·재생목록 링크는 받지 않습니다)."
        ));
    }
    rejected(URL_REJECT)
}

/// "URL 형태"인가. 공백이 섞인 문장은 URL이 아니라 텍스트다.
///
/// 맨 도메인(`example.com`)까지 URL로 보면 `파일.txt` 같은 평범한 텍스트가 거부되므로,
/// 스킴이 있거나 `www.`로 시작하거나 호스트가 YouTube일 때만 URL로 본다.
fn looks_like_url(s: &str) -> bool {
    if s.split_whitespace().nth(1).is_some() {
        return false;
    }
    if let Some(i) = s.find("://") {
        let scheme = &s[..i];
        return !scheme.is_empty()
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'));
    }
    s.len() > 4 && s[..4].eq_ignore_ascii_case("www.") || is_youtube_url(s)
}

/// 호스트가 YouTube인가 (스킴 유무 무관).
fn is_youtube_url(s: &str) -> bool {
    split_url(s)
        .map(|u| is_youtube_host(&u.host))
        .unwrap_or(false)
}

struct UrlParts {
    host: String,
    path: String,
    query: String,
}

/// `url` 크레이트를 쓰지 않고 필요한 만큼만 쪼갠다 (§: 새 의존성 금지).
fn split_url(s: &str) -> Option<UrlParts> {
    let rest = match s.find("://") {
        Some(i) => {
            let scheme = s[..i].to_ascii_lowercase();
            // http(s)가 아니면 YouTube 링크일 수 없다.
            if scheme != "http" && scheme != "https" {
                return None;
            }
            &s[i + 3..]
        }
        None => s.strip_prefix("//").unwrap_or(s),
    };
    // fragment는 버린다.
    let rest = rest.split('#').next().unwrap_or_default();
    let (authority, after) = match rest.find(['/', '?']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    // userinfo(`user:pw@host`) 제거. `@` 뒤가 진짜 호스트다.
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let host = authority
        .split(':')
        .next()
        .unwrap_or(authority)
        .to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    let (path, query) = match after.find('?') {
        Some(i) => (&after[..i], &after[i + 1..]),
        None => (after, ""),
    };
    Some(UrlParts {
        host,
        path: path.to_string(),
        query: query.to_string(),
    })
}

/// `notyoutube.com` 같은 접미사 위장을 막기 위해 라벨 경계를 확인한다.
fn is_youtube_host(host: &str) -> bool {
    let h = host.strip_prefix("www.").unwrap_or(host);
    h == "youtu.be"
        || h == "youtube.com"
        || h.ends_with(".youtube.com")
        || h == "youtube-nocookie.com"
        || h.ends_with(".youtube-nocookie.com")
}

/// YouTube URL에서 video id를 뽑는다.
/// `youtube.com/watch`·`youtu.be`·`/shorts/` 형태를 인식한다 (§9.3 단계 1).
pub fn youtube_video_id(url: &str) -> Option<String> {
    let u = split_url(url.trim())?;
    if !is_youtube_host(&u.host) {
        return None;
    }
    let segs: Vec<&str> = u.path.split('/').filter(|s| !s.is_empty()).collect();

    let candidate: Option<&str> = if u.host.strip_prefix("www.").unwrap_or(&u.host) == "youtu.be" {
        // https://youtu.be/<id>?si=...
        segs.first().copied()
    } else {
        match segs.first().copied() {
            // /shorts/<id> · /embed/<id> · /live/<id> · /v/<id> · /e/<id>
            Some("shorts") | Some("embed") | Some("live") | Some("v") | Some("e") => {
                segs.get(1).copied()
            }
            // /watch?v=<id> — 쿼리 파라미터가 여럿 섞여 있어도 v만 뽑는다.
            _ => query_param(&u.query, "v"),
        }
    };

    candidate.filter(|c| is_video_id(c)).map(str::to_string)
}

fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then_some(v)
    })
}

/// id는 11자 `[A-Za-z0-9_-]`.
fn is_video_id(s: &str) -> bool {
    s.len() == 11
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// 정규형 `https://www.youtube.com/watch?v=<id>` (§6.2 `source.origin`).
pub fn youtube_canonical_url(video_id: &str) -> String {
    format!("https://www.youtube.com/watch?v={video_id}")
}

// ---------------------------------------------------------------------------
// 파일 (단계 4~7)
// ---------------------------------------------------------------------------

/// 파일 판별 (단계 4~7). magic bytes + ffprobe.
pub fn detect_kind(path: &Path) -> DetectedKind {
    let head = match read_head(path) {
        Ok(h) => h,
        Err(reason) => return rejected(reason),
    };
    if head.is_empty() {
        return rejected("빈 파일입니다.");
    }

    // SVG는 텍스트라 magic bytes가 없고 infer는 `text/xml`로 본다.
    // §9.4가 벡터를 이미지로 받으므로 매처보다 먼저 확인한다 (T25).
    if looks_like_svg(&head) {
        return accepted(Kind::Image, "image/svg+xml");
    }

    match infer::get(&head) {
        Some(t) => match t.matcher_type() {
            // 단계 4 — magic bytes가 영상 컨테이너. 실제로 열어 확인한다 (T11f).
            infer::MatcherType::Video => classify_video_candidate(path, t.mime_type()),
            // 단계 5 — GIF·WebP도 여기로 온다. 애니메이션이어도 image다 (T11h·T25c).
            infer::MatcherType::Image => accepted(Kind::Image, t.mime_type()),
            // 단계 6 (T11g)
            infer::MatcherType::Audio => rejected(AUDIO_REJECT),
            // 단계 7
            _ => rejected(format!(
                "{} 형식은 받지 않습니다. 이미지·영상 파일이나 YouTube 링크를 넣어주세요.",
                t.mime_type()
            )),
        },
        // magic bytes를 모르는 경우. 확장자를 믿지 않으므로 컨테이너를 열어본다.
        None => classify_unknown(path, &head),
    }
}

/// magic bytes가 영상이라고 했을 때의 확정 절차.
fn classify_video_candidate(path: &Path, magic_mime: &str) -> DetectedKind {
    let Some(tools) = probe_tools() else {
        // ffprobe가 없으면 magic bytes 판정만으로 진행한다 (§9.7 1단계 부재).
        return accepted(Kind::Video, magic_mime);
    };
    match probe_ignoring_extension(&tools, path) {
        Ok(info) => {
            // §9.1 단계 4 예외 — 비디오 스트림이 있어도 GIF·애니메이션 WebP는 image다.
            if is_image_format(&info.format_name) {
                return accepted(Kind::Image, image_mime_for(&info.format_name, magic_mime));
            }
            if info.has_video {
                return accepted(Kind::Video, magic_mime);
            }
            if info.has_audio {
                // 컨테이너는 영상인데 실제로는 소리만 든 파일 (T11g와 같은 취급).
                return rejected(AUDIO_REJECT);
            }
            rejected(format!(
                "{magic_mime} 컨테이너를 열었지만 영상·오디오 스트림이 없습니다."
            ))
        }
        // magic bytes라는 근거가 이미 있으므로 판정을 유지한다.
        // 실제로 깨진 파일이면 이후 ffmpeg 처리에서 명령어와 stderr가 붙은 에러가 난다 (§15).
        Err(_) => accepted(Kind::Video, magic_mime),
    }
}

/// magic bytes를 모르는 파일. **확장자를 보지 않는다.**
///
/// 여기서는 근거가 ffprobe뿐이므로 보수적으로 간다. ffprobe는 내용 판별에 실패하면
/// **확장자로 데모서를 고르기 때문에**(`memo.png` 텍스트 파일이 `image2`로 열린다)
/// 알려진 영상 컨테이너일 때만 `video`로 인정한다.
fn classify_unknown(path: &Path, head: &[u8]) -> DetectedKind {
    const TEXT_HINT: &str = "텍스트 파일은 파일이 아니라 클립보드로 붙여넣어 주세요 (§9.2).";
    const NO_PROBE: &str =
        "파일 형식을 판별하지 못했습니다. ffmpeg/ffprobe가 없어 컨테이너를 열어볼 수 없습니다.";
    const UNKNOWN: &str =
        "형식을 판별하지 못했습니다. 이미지·영상 파일이나 YouTube 링크를 넣어주세요.";

    // 미디어 파일은 앞 8KB 안에 NUL이나 비UTF-8 바이트가 반드시 섞인다.
    // 텍스트로 보이면 ffprobe에 물어볼 것도 없다.
    if looks_like_text(head) {
        return rejected(TEXT_HINT);
    }

    let Some(tools) = probe_tools() else {
        return rejected(NO_PROBE);
    };

    match probe_ignoring_extension(&tools, path) {
        Ok(info) if info.has_video && is_known_container(&info.format_name) => {
            accepted(Kind::Video, container_mime(&info.format_name))
        }
        Ok(info) if info.has_audio && !info.has_video => rejected(AUDIO_REJECT),
        Ok(info) => rejected(format!(
            "형식을 판별하지 못했습니다 (ffprobe: {}). 이미지·영상 파일이나 YouTube 링크를 넣어주세요.",
            if info.format_name.is_empty() {
                "알 수 없음"
            } else {
                &info.format_name
            }
        )),
        Err(_) => rejected(UNKNOWN),
    }
}

/// ffprobe에 **확장자를 감추고** 물어본다 (§9.1 "확장자를 신뢰하지 않는다").
///
/// ffprobe는 내용 판별 점수가 낮으면 확장자로 데모서를 고른다. 확장자 없는 심볼릭 링크를
/// 물리면 내용만으로 판별한다. 링크를 만들지 못하면 원본 경로로 폴백한다.
fn probe_ignoring_extension(
    tools: &crate::ffmpeg::FfmpegTools,
    path: &Path,
) -> soul_core::error::Result<crate::ffmpeg::ProbeInfo> {
    #[cfg(unix)]
    {
        if let Ok(dir) = tempfile::tempdir() {
            let link = dir.path().join("probe-target");
            let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
            if std::os::unix::fs::symlink(&target, &link).is_ok() {
                // `dir`은 이 표현식이 끝난 뒤에 정리되므로 probe 동안 살아 있다.
                return crate::ffmpeg::probe(tools, &link);
            }
        }
    }
    crate::ffmpeg::probe(tools, path)
}

/// ffprobe `format_name`이 정지·애니메이션 이미지인가.
/// GIF·APNG·애니메이션 WebP는 비디오 스트림을 갖지만 image다 (§9.1 단계 4 예외).
fn is_image_format(format_name: &str) -> bool {
    format_name.split(',').any(|f| {
        let f = f.trim();
        f.ends_with("_pipe") // png_pipe · webp_pipe · jpeg_pipe …
            || matches!(f, "gif" | "apng" | "webp" | "image2" | "image2pipe" | "svg")
    })
}

/// ffprobe format이 우리가 아는 영상 컨테이너인가 (magic bytes 근거가 없을 때만 쓴다).
fn is_known_container(format_name: &str) -> bool {
    const CONTAINERS: &[&str] = &[
        "mov",
        "mp4",
        "m4a",
        "3gp",
        "3g2",
        "mj2",
        "matroska",
        "webm",
        "avi",
        "asf",
        "flv",
        "mpeg",
        "mpegts",
        "mpegvideo",
        "ogg",
        "ogv",
        "nut",
        "mxf",
        "dv",
        "y4m",
        "vob",
        "rm",
        "ivf",
        "h264",
        "hevc",
        "av1",
    ];
    format_name
        .split(',')
        .any(|f| CONTAINERS.contains(&f.trim()))
}

fn container_mime(format_name: &str) -> String {
    let first = format_name.split(',').next().unwrap_or_default().trim();
    if first.is_empty() {
        "video/x-unknown".to_string()
    } else {
        format!("video/{first}")
    }
}

fn image_mime_for(format_name: &str, fallback: &str) -> String {
    for f in format_name.split(',') {
        match f.trim() {
            "gif" => return "image/gif".to_string(),
            "webp" | "webp_pipe" => return "image/webp".to_string(),
            "apng" | "png_pipe" => return "image/png".to_string(),
            _ => {}
        }
    }
    fallback.to_string()
}

/// SVG는 XML 선언·주석·BOM이 앞에 붙을 수 있다. 앞부분에 `<svg`가 있으면 SVG로 본다.
fn looks_like_svg(head: &[u8]) -> bool {
    let n = head.len().min(1024);
    let Ok(s) = std::str::from_utf8(&head[..n]) else {
        // 잘린 멀티바이트 때문에 실패했을 수 있으므로 손실 변환으로 한 번 더 본다.
        return String::from_utf8_lossy(&head[..n])
            .to_ascii_lowercase()
            .contains("<svg");
    };
    s.to_ascii_lowercase().contains("<svg")
}

/// NUL이 없고 유효한 UTF-8이며 제어문자가 드물면 텍스트로 본다.
fn looks_like_text(head: &[u8]) -> bool {
    if head.contains(&0) {
        return false;
    }
    let n = head.len().min(4096);
    let s = match std::str::from_utf8(&head[..n]) {
        Ok(s) => s,
        Err(e) => {
            // 멀티바이트가 잘린 것뿐이면 그 앞까지는 유효한 텍스트다.
            if e.error_len().is_some() {
                return false;
            }
            std::str::from_utf8(&head[..e.valid_up_to()]).unwrap_or_default()
        }
    };
    let total = s.chars().count();
    if total == 0 {
        return false;
    }
    let ctrl = s
        .chars()
        .filter(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        .count();
    ctrl * 20 < total
}

fn read_head(path: &Path) -> std::result::Result<Vec<u8>, String> {
    use std::io::Read;
    let md = std::fs::metadata(path)
        .map_err(|e| format!("파일을 읽을 수 없습니다: {} ({e})", path.display()))?;
    if md.is_dir() {
        return Err(format!("디렉토리는 받지 않습니다: {}", path.display()));
    }
    let mut f = std::fs::File::open(path)
        .map_err(|e| format!("파일을 열 수 없습니다: {} ({e})", path.display()))?;
    let mut buf = vec![0u8; HEAD_BYTES];
    let mut filled = 0usize;
    loop {
        match f.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => {
                filled += n;
                if filled == buf.len() {
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(format!("파일을 읽을 수 없습니다: {} ({e})", path.display())),
        }
    }
    buf.truncate(filled);
    Ok(buf)
}

/// 판별용 ffprobe. 없으면 magic bytes만으로 간다.
fn probe_tools() -> Option<crate::ffmpeg::FfmpegTools> {
    let bin = soul_core::paths::Paths::discover()
        .map(|p| p.bin())
        .unwrap_or_else(|_| std::path::PathBuf::from("bin"));
    crate::ffmpeg::locate(&bin)
}

fn accepted(kind: Kind, mime: impl Into<String>) -> DetectedKind {
    DetectedKind::Accepted {
        kind,
        mime: mime.into(),
    }
}

fn rejected(reason: impl Into<String>) -> DetectedKind {
    DetectedKind::Rejected {
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const ID: &str = "dQw4w9WgXcQ";

    fn write_file(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(bytes).unwrap();
        p
    }

    fn kind_of(d: &DetectedKind) -> Option<Kind> {
        match d {
            DetectedKind::Accepted { kind, .. } => Some(*kind),
            _ => None,
        }
    }

    // ---------------- §9.3 단계 1 — URL 형태 ----------------

    #[test]
    fn youtube_id를_여러_형태에서_뽑는다() {
        let ok = [
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "http://youtube.com/watch?v=dQw4w9WgXcQ",
            "https://www.youtube.com/watch?app=desktop&v=dQw4w9WgXcQ&list=RDxx",
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=42s",
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ#t=10",
            "https://youtu.be/dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ?si=AbCdEfGhIjKl",
            "youtu.be/dQw4w9WgXcQ",
            "https://www.youtube.com/shorts/dQw4w9WgXcQ",
            "https://www.youtube.com/shorts/dQw4w9WgXcQ?feature=share",
            "https://www.youtube.com/embed/dQw4w9WgXcQ?rel=0",
            "https://m.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://music.youtube.com/watch?v=dQw4w9WgXcQ&list=RDAMVM",
            "https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ",
            "https://www.youtube.com/live/dQw4w9WgXcQ",
            "  https://youtu.be/dQw4w9WgXcQ  ",
        ];
        for u in ok {
            assert_eq!(youtube_video_id(u).as_deref(), Some(ID), "{u}");
        }
    }

    #[test]
    fn youtube가_아니거나_id가_없으면_none이다() {
        let no = [
            "https://vimeo.com/12345678",
            "https://example.com/watch?v=dQw4w9WgXcQ",
            // 접미사 위장
            "https://youtube.com.evil.example/watch?v=dQw4w9WgXcQ",
            "https://notyoutube.com/watch?v=dQw4w9WgXcQ",
            // id 길이·문자 위반
            "https://www.youtube.com/watch?v=short",
            "https://www.youtube.com/watch?v=dQw4w9WgXcQextra",
            "https://www.youtube.com/watch?v=dQw4w9WgXc!",
            // 영상이 아닌 페이지
            "https://www.youtube.com/@somechannel",
            "https://www.youtube.com/playlist?list=PL1234567890",
            "그냥 텍스트",
        ];
        for u in no {
            assert_eq!(youtube_video_id(u), None, "{u}");
        }
    }

    #[test]
    fn canonical_url은_정규형이다() {
        assert_eq!(
            youtube_canonical_url(ID),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        );
        let d = detect_clipboard("https://youtu.be/dQw4w9WgXcQ?si=x");
        assert_eq!(
            d,
            DetectedKind::YouTube {
                video_id: ID.to_string(),
                canonical_url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ".to_string(),
            }
        );
    }

    // ---------------- §9.1 단계 1~3 ----------------

    #[test]
    fn 단계1_youtube_url은_youtube로_간다() {
        assert!(matches!(
            detect_clipboard("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
            DetectedKind::YouTube { .. }
        ));
    }

    /// T11c — YouTube 외 URL 붙여넣기 → 거부.
    #[test]
    fn t11c_단계2_youtube가_아닌_url은_거부한다() {
        for u in [
            "https://example.com/article",
            "http://vimeo.com/12345678",
            "https://soundcloud.com/artist/track",
            "www.example.com/x",
            "ftp://files.example.com/a.mp4",
        ] {
            match detect_clipboard(u) {
                DetectedKind::Rejected { reason } => {
                    assert!(reason.contains("YouTube 링크만 받습니다"), "{u}: {reason}")
                }
                other => panic!("{u} → {other:?}"),
            }
        }
    }

    #[test]
    fn 단계2_youtube_도메인이지만_영상이_아니면_사유를_따로_준다() {
        match detect_clipboard("https://www.youtube.com/@somechannel") {
            DetectedKind::Rejected { reason } => {
                assert!(reason.contains("영상 id"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn 단계3_url이_아니면_text다() {
        for t in [
            "오늘 읽은 문장",
            "file.txt",                              // 점이 있어도 URL이 아니다
            "https://example.com 을 봤다",           // 공백이 섞이면 문장이다
            "여러 줄\nhttps://youtu.be/dQw4w9WgXcQ", // 링크가 섞인 문장도 텍스트
            "3.14",
        ] {
            assert_eq!(kind_of(&detect_clipboard(t)), Some(Kind::Text), "{t}");
        }
    }

    #[test]
    fn 빈_클립보드는_거부한다() {
        assert!(matches!(
            detect_clipboard("   \n "),
            DetectedKind::Rejected { .. }
        ));
    }

    // ---------------- §9.1 단계 4~7 (magic bytes) ----------------

    /// T11h·T25c — GIF는 비디오 스트림이 있어도 image다. 확장자가 .mp4여도 마찬가지.
    #[test]
    fn t25c_gif는_확장자가_mp4여도_image다() {
        let dir = tempfile::tempdir().unwrap();
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&[0u8; 64]);
        let p = write_file(dir.path(), "animation.mp4", &gif);
        assert_eq!(
            detect_kind(&p),
            accepted(Kind::Image, "image/gif"),
            "GIF는 언제나 image (§9.4 첫 프레임 규칙)"
        );
    }

    /// T11h — 애니메이션 WebP도 image.
    #[test]
    fn t11h_애니메이션_webp는_image다() {
        let dir = tempfile::tempdir().unwrap();
        // RIFF <size> WEBP VP8X (VP8X = 확장 포맷 = 애니메이션 가능)
        let mut webp = Vec::new();
        webp.extend_from_slice(b"RIFF");
        webp.extend_from_slice(&[0x40, 0, 0, 0]);
        webp.extend_from_slice(b"WEBPVP8X");
        webp.extend_from_slice(&[0u8; 64]);
        let p = write_file(dir.path(), "anim.webm", &webp);
        assert_eq!(detect_kind(&p), accepted(Kind::Image, "image/webp"));
    }

    /// T11g — 로컬 오디오는 거부하고 YouTube로 안내한다.
    #[test]
    fn t11g_오디오_파일은_거부하고_youtube로_안내한다() {
        let dir = tempfile::tempdir().unwrap();
        // ID3 헤더가 붙은 mp3. 확장자는 일부러 .txt.
        let mut mp3 = b"ID3\x03\x00\x00\x00\x00\x00\x00".to_vec();
        mp3.extend_from_slice(&[0u8; 64]);
        let p = write_file(dir.path(), "song.txt", &mp3);
        match detect_kind(&p) {
            DetectedKind::Rejected { reason } => {
                assert!(reason.contains("YouTube 링크로"), "{reason}")
            }
            other => panic!("{other:?}"),
        }
    }

    /// T11f — 확장자가 .txt여도 magic bytes가 mp4면 video.
    #[test]
    fn t11f_확장자가_txt여도_mp4_magic이면_video다() {
        let dir = tempfile::tempdir().unwrap();
        let mut mp4 = Vec::new();
        mp4.extend_from_slice(&[0, 0, 0, 0x20]);
        mp4.extend_from_slice(b"ftypisom");
        mp4.extend_from_slice(b"\x00\x00\x02\x00isomiso2avc1mp41");
        mp4.extend_from_slice(&[0u8; 64]);
        let p = write_file(dir.path(), "notes.txt", &mp4);
        assert_eq!(kind_of(&detect_kind(&p)), Some(Kind::Video));
    }

    #[test]
    fn png은_확장자가_없어도_image다() {
        let dir = tempfile::tempdir().unwrap();
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        png.extend_from_slice(&[0u8; 64]);
        let p = write_file(dir.path(), "noext", &png);
        assert_eq!(detect_kind(&p), accepted(Kind::Image, "image/png"));
    }

    #[test]
    fn svg는_magic_bytes가_없어도_image다() {
        let dir = tempfile::tempdir().unwrap();
        let svg = br#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"/>"#;
        let p = write_file(dir.path(), "vector.dat", svg);
        assert_eq!(detect_kind(&p), accepted(Kind::Image, "image/svg+xml"));
    }

    /// 확장자가 .png인 **텍스트** 파일. ffprobe는 확장자로 데모서를 골라 image2로 열지만
    /// 우리는 거부해야 한다 (확장자 불신은 ffprobe에게도 적용된다).
    #[test]
    fn 평범한_텍스트_파일은_확장자가_png여도_거부한다() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_file(
            dir.path(),
            "memo.png",
            "오늘 읽은 문장\n".repeat(20).as_bytes(),
        );
        match detect_kind(&p) {
            DetectedKind::Rejected { reason } => assert!(reason.contains("클립보드"), "{reason}"),
            other => panic!("텍스트 파일은 거부해야 한다: {other:?}"),
        }
    }

    #[test]
    fn 빈_파일과_없는_파일과_디렉토리는_거부한다() {
        let dir = tempfile::tempdir().unwrap();
        let empty = write_file(dir.path(), "empty.jpg", b"");
        assert!(matches!(detect_kind(&empty), DetectedKind::Rejected { .. }));
        assert!(matches!(
            detect_kind(&dir.path().join("없는파일.png")),
            DetectedKind::Rejected { .. }
        ));
        assert!(matches!(
            detect_kind(dir.path()),
            DetectedKind::Rejected { .. }
        ));
    }

    #[test]
    fn pdf는_거부하고_사유를_보여준다() {
        let dir = tempfile::tempdir().unwrap();
        let mut pdf = b"%PDF-1.7\n".to_vec();
        pdf.extend_from_slice(&[0u8; 64]);
        let p = write_file(dir.path(), "paper.jpg", &pdf);
        match detect_kind(&p) {
            DetectedKind::Rejected { reason } => assert!(reason.contains("pdf"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }

    // ---------------- format_name 규칙 ----------------

    #[test]
    fn format_name_분류() {
        assert!(is_image_format("gif"));
        assert!(is_image_format("webp_pipe"));
        assert!(is_image_format("png_pipe"));
        assert!(!is_image_format("mov,mp4,m4a,3gp,3g2,mj2"));
        assert!(!is_image_format("matroska,webm"));

        assert!(is_known_container("mov,mp4,m4a,3gp,3g2,mj2"));
        assert!(is_known_container("matroska,webm"));
        assert!(!is_known_container("tty")); // 텍스트 파일을 영상으로 오판하지 않는다
        assert!(!is_known_container("srt"));

        assert_eq!(image_mime_for("gif", "x"), "image/gif");
        assert_eq!(image_mime_for("webp_pipe", "x"), "image/webp");
        assert_eq!(container_mime("matroska,webm"), "video/matroska");
    }

    // ---------------- 실제 ffmpeg 픽스처 (없으면 건너뜀) ----------------

    fn tools_or_skip() -> Option<crate::ffmpeg::FfmpegTools> {
        crate::ffmpeg::locate(Path::new("/nonexistent-bin-dir"))
    }

    /// T11f — 진짜 mp4를 만들어 확장자만 .txt로 바꾼다.
    #[test]
    fn t11f_실제_mp4를_txt_확장자로_줘도_video다() {
        let Some(tools) = tools_or_skip() else {
            return; // ffmpeg 없음 — 건너뜀
        };
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("clip.txt");
        crate::ffmpeg::run(
            &tools,
            &[
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=2:size=320x240:rate=10",
                "-f",
                "mp4",
                out.to_str().unwrap(),
            ],
        )
        .unwrap();
        assert_eq!(kind_of(&detect_kind(&out)), Some(Kind::Video));
    }

    /// 확장자가 이미지(.png)인 진짜 mp4. ffprobe에 확장자를 감춰야 video로 잡힌다.
    #[test]
    fn 실제_mp4를_png_확장자로_줘도_video다() {
        let Some(tools) = tools_or_skip() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("photo.png");
        crate::ffmpeg::run(
            &tools,
            &[
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=1:size=160x120:rate=10",
                "-f",
                "mp4",
                out.to_str().unwrap(),
            ],
        )
        .unwrap();
        assert_eq!(kind_of(&detect_kind(&out)), Some(Kind::Video));
    }

    /// mp4 컨테이너인데 소리만 든 파일 → 단계 6과 같은 취급 (T11g).
    #[test]
    fn 소리만_든_mp4는_오디오로_거부한다() {
        let Some(tools) = tools_or_skip() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("sound.mp4");
        crate::ffmpeg::run(
            &tools,
            &[
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=1",
                "-c:a",
                "aac",
                out.to_str().unwrap(),
            ],
        )
        .unwrap();
        match detect_kind(&out) {
            DetectedKind::Rejected { reason } => {
                assert!(reason.contains("YouTube 링크로"), "{reason}")
            }
            other => panic!("{other:?}"),
        }
    }

    /// T25c — 실제 애니메이션 GIF(비디오 스트림 있음)도 image다.
    #[test]
    fn t25c_실제_애니메이션_gif도_image다() {
        let Some(tools) = tools_or_skip() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("anim.gif");
        crate::ffmpeg::run(
            &tools,
            &[
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=1:size=64x64:rate=5",
                out.to_str().unwrap(),
            ],
        )
        .unwrap();
        // ffprobe로 보면 비디오 스트림이 있다. 그래도 image여야 한다.
        let info = crate::ffmpeg::probe(&tools, &out).unwrap();
        assert!(info.has_video, "GIF는 ffprobe상 비디오 스트림을 갖는다");
        assert_eq!(kind_of(&detect_kind(&out)), Some(Kind::Image));
    }
}
