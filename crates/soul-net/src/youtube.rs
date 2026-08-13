//! YouTube 해석 (§9.3) — **에이전트가 아니라 결정론적 핸들러다.**
//!
//! 판단할 것이 없으므로 고정 순서로 시도하고 실패하면 다음 단계를 건너뛴다.
//!
//! | 단계 | 하는 일 | 실패 시 |
//! |---|---|---|
//! | 1 | URL에서 video id 추출 | 입력 거부 |
//! | 2 | oEmbed로 제목·채널 획득. 키 불필요 | 3으로 진행 |
//! | 3 | 썸네일 URL 확정. `maxresdefault.jpg` → 404면 `hqdefault.jpg` | — |
//! | 4 | `youtube.api_key`가 있으면 Data API로 설명·태그·카테고리·길이 | 건너뜀 |
//! | 5 | `youtube.download_enabled`면 yt-dlp로 앞 30초만 | 6으로 폴백 |
//! | 6 | 다운로드 실패 시 **썸네일 + 메타데이터만으로** 서술, `quality: minimal` | — |
//!
//! ## kind 추정
//!
//! Data API 카테고리가 `10`(Music)이면 `audio`, 그 외는 `video`로 **추정한다** (T11d).
//! 카테고리를 못 얻으면 `video`. 화면 2 카드에서 **한 번의 탭으로 뒤집을 수 있다.**
//!
//! ## yt-dlp
//!
//! - **번들하지 않는다.** 몇 주면 낡는다 (§9.3)
//! - 봇 확인에 걸리면 `--cookies-from-browser chrome`으로 **한 번** 재시도
//! - 두 번 실패하면 **조용히** 단계 6으로. 사용자에게 오류를 띄우지 않는다 (T11)
//! - `download_enabled = false`가 기본값 (T11b)
//!
//! ## 썸네일
//!
//! 서술용으로는 **URL을 그대로 넘긴다** — OpenAI 서버가 대신 가져오므로 사용자 IP가
//! YouTube 썸네일 서버에 닿지 않는다 (§9.3 · §D6). 아카이브용 로컬 사본만 1회 내려받는다 (T11e).
//!
//! ## 구현 메모
//!
//! - `resolve`는 **단계 1을 제외하면 절대 Err를 내지 않는다.** 2·3·4가 전부 실패해도
//!   `video_id` · `canonical_url` · `thumbnail_url`은 채워서 돌려준다. 그 세 개만 있으면
//!   §9.3 단계 6(minimal 서술)이 성립한다.
//! - `download_clip`은 **어떤 경우에도 Err를 내지 않는다.** yt-dlp 부재·봇 확인·타임아웃·
//!   디스크 오류 전부 `Ok(None)`이다 (T11).

use soul_core::error::{Result, SoulError};
use soul_core::obs::Kind;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 평범한 브라우저 UA. 낯선 UA는 봇 확인을 더 자주 부른다.
const UA: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) \
     Chrome/124.0.0.0 Safari/537.36";

/// yt-dlp 한 번 실행의 상한. 30초 클립이 이보다 오래 걸리면 포기하고 단계 6으로 간다.
const YTDLP_TIMEOUT_SECS: u64 = 180;

/// 다운로드 중간 산출물. 캐시 조회에서 제외한다.
const PARTIAL_EXTS: [&str; 4] = ["part", "ytdl", "temp", "tmp"];

#[derive(Clone, Debug, Default)]
pub struct YoutubeMeta {
    pub video_id: String,
    pub canonical_url: String,
    pub title: Option<String>,
    pub channel: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub category_id: Option<String>,
    pub duration_secs: Option<f64>,
    /// 서술 호출에 **URL 그대로** 넘길 주소.
    pub thumbnail_url: String,
}

impl YoutubeMeta {
    /// 카테고리 10(Music) → `audio`, 그 외/미상 → `video` (§9.3).
    pub fn guess_kind(&self) -> Kind {
        match self.category_id.as_deref() {
            Some("10") => Kind::Audio,
            _ => Kind::Video,
        }
    }
}

/// video id로 쓸 수 있는 문자열인가. YouTube id는 `[A-Za-z0-9_-]`뿐이다.
///
/// 경로·명령줄·URL에 그대로 들어가므로 여기서 막지 않으면 인젝션 표면이 된다.
pub fn is_valid_video_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 32
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// 정규형 URL (§6.2 `source.origin`). `soul-media::probe`와 같은 형식이다.
fn canonical_url(video_id: &str) -> String {
    format!("https://www.youtube.com/watch?v={video_id}")
}

/// `https://i.ytimg.com/vi/<id>/<name>.jpg`
fn thumb_url(video_id: &str, name: &str) -> String {
    format!("https://i.ytimg.com/vi/{video_id}/{name}.jpg")
}

fn http(timeout_secs: u64) -> Result<reqwest::Client> {
    let mut b = reqwest::Client::builder().user_agent(UA);
    if timeout_secs > 0 {
        b = b.timeout(Duration::from_secs(timeout_secs));
    }
    b.build()
        .map_err(|e| SoulError::invalid(format!("HTTP 클라이언트 생성 실패: {e}")))
}

/// ISO 8601 기간(`PT4M13S`)을 초로. Data API `contentDetails.duration`이 이 형식이다.
///
/// 날짜부는 `W`·`D`만 받는다. `Y`·`M`은 길이가 달력에 의존해 초로 환산할 수 없고,
/// 영상 길이에는 나타나지도 않는다. 그런 값은 `None`이다 — 지어내지 않는다.
pub fn parse_iso8601_duration(s: &str) -> Option<f64> {
    let s = s.trim();
    let rest = s.strip_prefix('P').or_else(|| s.strip_prefix('p'))?;
    let mut total = 0.0f64;
    let mut num = String::new();
    let mut in_time = false;
    let mut seen_any = false;

    for ch in rest.chars() {
        match ch {
            'T' | 't' => {
                if in_time || !num.is_empty() {
                    return None; // `P1T` 처럼 숫자가 남은 채 T가 오면 형식 오류
                }
                in_time = true;
            }
            '0'..='9' | '.' | ',' => num.push(if ch == ',' { '.' } else { ch }),
            _ => {
                if num.is_empty() {
                    return None;
                }
                let v: f64 = num.parse().ok()?;
                num.clear();
                let mult = match (in_time, ch.to_ascii_uppercase()) {
                    (false, 'W') => 604_800.0,
                    (false, 'D') => 86_400.0,
                    (true, 'H') => 3_600.0,
                    (true, 'M') => 60.0,
                    (true, 'S') => 1.0,
                    _ => return None,
                };
                total += v * mult;
                seen_any = true;
            }
        }
    }
    if !num.is_empty() || !seen_any {
        return None;
    }
    Some(total)
}

/// 단계 1~4. 네트워크는 oEmbed와 Data API만 쓴다.
pub async fn resolve(
    video_id: &str,
    api_key: Option<&str>,
    timeout_secs: u64,
) -> Result<YoutubeMeta> {
    // 단계 1 — 여기서만 거부한다. 이후 단계는 실패해도 진행한다.
    if !is_valid_video_id(video_id) {
        return Err(SoulError::invalid(format!(
            "YouTube video id가 아닙니다: {video_id}"
        )));
    }

    let canonical = canonical_url(video_id);
    let mut meta = YoutubeMeta {
        video_id: video_id.to_string(),
        canonical_url: canonical.clone(),
        // hqdefault는 항상 존재한다. 단계 3이 통째로 실패해도 이 값은 유효하다.
        thumbnail_url: thumb_url(video_id, "hqdefault"),
        ..YoutubeMeta::default()
    };

    let client = match http(timeout_secs) {
        Ok(c) => c,
        Err(_) => return Ok(meta), // 클라이언트조차 못 만들면 단계 6 재료만 돌려준다
    };

    // 단계 2 — oEmbed. 키 불필요. 실패해도 3으로 간다.
    if let Some(v) = oembed(&client, &canonical).await {
        meta.title = nonempty(v.get("title").and_then(|x| x.as_str()));
        meta.channel = nonempty(v.get("author_name").and_then(|x| x.as_str()));
    }

    // 단계 3 — maxres가 있으면 그것, 없으면 hqdefault.
    let maxres = thumb_url(video_id, "maxresdefault");
    if head_ok(&client, &maxres).await {
        meta.thumbnail_url = maxres;
    }

    // 단계 4 — 키가 있을 때만. 없으면 건너뛴다.
    if let Some(key) = api_key.map(str::trim).filter(|k| !k.is_empty()) {
        if let Some(item) = data_api(&client, video_id, key).await {
            let snippet = item.get("snippet");
            if meta.title.is_none() {
                meta.title = nonempty(
                    snippet
                        .and_then(|s| s.get("title"))
                        .and_then(|x| x.as_str()),
                );
            }
            if meta.channel.is_none() {
                meta.channel = nonempty(
                    snippet
                        .and_then(|s| s.get("channelTitle"))
                        .and_then(|x| x.as_str()),
                );
            }
            meta.description = nonempty(
                snippet
                    .and_then(|s| s.get("description"))
                    .and_then(|x| x.as_str()),
            );
            meta.category_id = nonempty(
                snippet
                    .and_then(|s| s.get("categoryId"))
                    .and_then(|x| x.as_str()),
            );
            if let Some(tags) = snippet
                .and_then(|s| s.get("tags"))
                .and_then(|x| x.as_array())
            {
                meta.tags = tags
                    .iter()
                    .filter_map(|t| t.as_str())
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect();
            }
            meta.duration_secs = item
                .get("contentDetails")
                .and_then(|c| c.get("duration"))
                .and_then(|d| d.as_str())
                .and_then(parse_iso8601_duration);
        }
    }

    Ok(meta)
}

fn nonempty(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// 단계 2. 어떤 실패도 `None`이다 — 호출자는 그냥 다음 단계로 간다.
async fn oembed(client: &reqwest::Client, canonical: &str) -> Option<serde_json::Value> {
    let url = url::Url::parse_with_params(
        "https://www.youtube.com/oembed",
        &[("url", canonical), ("format", "json")],
    )
    .ok()?;
    let resp = client.get(url.as_str()).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json::<serde_json::Value>().await.ok()
}

/// 단계 3. HEAD가 막힌 서버면 `false`로 떨어져 hqdefault를 쓴다 — 항상 존재하므로 안전하다.
async fn head_ok(client: &reqwest::Client, url: &str) -> bool {
    match client.head(url).send().await {
        Ok(r) => r.status().is_success(),
        Err(_) => false,
    }
}

/// 단계 4. `items[0]`를 돌려준다. 키가 틀렸거나 영상이 비공개면 `None`.
async fn data_api(
    client: &reqwest::Client,
    video_id: &str,
    api_key: &str,
) -> Option<serde_json::Value> {
    let url = url::Url::parse_with_params(
        "https://www.googleapis.com/youtube/v3/videos",
        &[
            ("part", "snippet,contentDetails"),
            ("id", video_id),
            ("key", api_key),
        ],
    )
    .ok()?;
    let resp = client.get(url.as_str()).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let v: serde_json::Value = resp.json().await.ok()?;
    v.get("items")?.as_array()?.first().cloned()
}

#[derive(Clone, Debug)]
pub struct DownloadedClip {
    pub path: std::path::PathBuf,
    pub kind: Kind,
}

/// `-f` 값 (§9.3). `audio`는 오디오 스트림만 받는다 — 정지화면 30장을 뽑는 낭비를 막는다.
pub fn format_selector(kind: Kind) -> &'static str {
    match kind {
        Kind::Audio => "ba",
        _ => "bv*[height<=720]+ba/b[height<=720]",
    }
}

/// `--download-sections` 값. 항상 처음부터 `seconds`초까지다.
pub fn section_arg(seconds: u32) -> String {
    format!("*0-{}", seconds.max(1))
}

/// yt-dlp stderr가 봇 확인·로그인 요구인가. 이때만 쿠키로 **한 번** 재시도한다 (§9.3).
///
/// 그 외 실패(포맷 없음·삭제된 영상·네트워크)는 쿠키를 붙여도 결과가 같으므로 재시도하지 않는다.
pub fn looks_like_bot_check(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    [
        "not a bot",
        "sign in to confirm",
        "confirm your age",
        "age-restricted",
        "login required",
        "--cookies",
        "cookies-from-browser",
        "please sign in",
    ]
    .iter()
    .any(|needle| s.contains(needle))
}

/// 캐시에 이미 받아둔 클립 (T11j). 파일명은 `<video_id>.<kind>.<ext>`.
///
/// `soul recast`로 kind를 뒤집어도 같은 kind로 되돌아오면 yt-dlp를 다시 부르지 않는다.
fn cached_clip(cache_dir: &Path, video_id: &str, kind: Kind) -> Option<PathBuf> {
    let prefix = format!("{video_id}.{}.", kind.as_str());
    let mut found: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(cache_dir).ok()?.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if !name.starts_with(&prefix) {
            continue;
        }
        let ext = name[prefix.len()..].to_ascii_lowercase();
        // `.mp4.part` 같은 중간 산출물은 클립이 아니다
        if ext.is_empty()
            || PARTIAL_EXTS
                .iter()
                .any(|p| ext == *p || ext.ends_with(&format!(".{p}")))
        {
            continue;
        }
        if entry.metadata().map(|m| m.len() == 0).unwrap_or(true) {
            continue;
        }
        found.push(path);
    }
    found.sort();
    found.into_iter().next()
}

/// yt-dlp 한 번 실행. `None`은 **실행 자체가 불가능**(PATH에 없음·타임아웃)이라는 뜻이다.
async fn run_ytdlp(
    video_id: &str,
    kind: Kind,
    cache_dir: &Path,
    seconds: u32,
    cookies_browser: Option<&str>,
) -> Option<std::process::Output> {
    let mut cmd = tokio::process::Command::new("yt-dlp");
    cmd.arg("--no-playlist")
        .arg("--no-progress")
        .arg("--no-warnings")
        .arg("--download-sections")
        .arg(section_arg(seconds))
        .arg("-f")
        .arg(format_selector(kind))
        .arg("-o")
        .arg(cache_dir.join(format!("{video_id}.{}.%(ext)s", kind.as_str())));
    if let Some(browser) = cookies_browser {
        cmd.arg("--cookies-from-browser").arg(browser);
    }
    cmd.arg(canonical_url(video_id));
    // 타임아웃으로 future를 버릴 때 자식 프로세스가 남지 않게 한다
    cmd.kill_on_drop(true);

    match tokio::time::timeout(Duration::from_secs(YTDLP_TIMEOUT_SECS), cmd.output()).await {
        Ok(Ok(out)) => Some(out),
        // spawn 실패(PATH에 없음 포함)도, 타임아웃도 "yt-dlp를 쓸 수 없다"로 같게 다룬다
        Ok(Err(_)) | Err(_) => None,
    }
}

/// 단계 5. 실패하면 `Ok(None)` — **에러가 아니다.** 정상 경로다 (§15, T11).
///
/// 이미 캐시에 클립이 있으면 yt-dlp를 부르지 않는다 (T11j).
pub async fn download_clip(
    video_id: &str,
    kind: Kind,
    cache_dir: &std::path::Path,
    seconds: u32,
) -> Result<Option<DownloadedClip>> {
    if !is_valid_video_id(video_id) {
        return Ok(None);
    }

    // T11j — 캐시 우선. yt-dlp를 부르지 않는다.
    if let Some(path) = cached_clip(cache_dir, video_id, kind) {
        return Ok(Some(DownloadedClip { path, kind }));
    }

    if tokio::fs::create_dir_all(cache_dir).await.is_err() {
        return Ok(None);
    }

    let ok = match run_ytdlp(video_id, kind, cache_dir, seconds, None).await {
        // yt-dlp가 없거나 못 돌았다 → 조용히 단계 6
        None => return Ok(None),
        Some(out) if out.status.success() => true,
        Some(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            if looks_like_bot_check(&stderr) {
                // 쿠키로 **한 번만** 재시도 (§9.3)
                match run_ytdlp(video_id, kind, cache_dir, seconds, Some("chrome")).await {
                    Some(retry) => retry.status.success(),
                    None => false,
                }
            } else {
                false
            }
        }
    };

    if !ok {
        // 두 번 실패 = 단계 6. 사용자에게 오류를 띄우지 않는다 (T11).
        return Ok(None);
    }

    Ok(cached_clip(cache_dir, video_id, kind).map(|path| DownloadedClip { path, kind }))
}

/// 아카이브 표시용 로컬 썸네일 사본 (§20.4). **1회만 내려받는다.**
///
/// 재시도하지 않는다 — 서술 경로는 URL을 그대로 넘기므로(§9.3) 이 사본이 없어도
/// 파이프라인은 진행된다. 여러 번 부르지 않는 것은 호출자(투입 1회)의 책임이다.
pub async fn fetch_thumbnail(url: &str, timeout_secs: u64) -> Result<Vec<u8>> {
    let client = http(timeout_secs)?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| SoulError::invalid(format!("썸네일 요청 실패: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(SoulError::invalid(format!("썸네일 응답 {status}: {url}")));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| SoulError::invalid(format!("썸네일 본문 읽기 실패: {e}")))?;
    if bytes.is_empty() {
        return Err(SoulError::invalid(format!("빈 썸네일 응답: {url}")));
    }
    Ok(bytes.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 의존성 없이 쓰는 테스트용 임시 디렉토리.
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let p =
                std::env::temp_dir().join(format!("soul-net-{tag}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn touch(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn video_id_charset_is_enforced() {
        assert!(is_valid_video_id("dQw4w9WgXcQ"));
        assert!(is_valid_video_id("_-a9Z0"));
        assert!(!is_valid_video_id(""));
        assert!(!is_valid_video_id("../../etc/passwd"));
        assert!(!is_valid_video_id("id with space"));
        assert!(!is_valid_video_id("id;rm -rf /"));
        assert!(!is_valid_video_id(&"a".repeat(33)));
    }

    #[test]
    fn canonical_and_thumbnail_urls() {
        assert_eq!(
            canonical_url("dQw4w9WgXcQ"),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        );
        assert_eq!(
            thumb_url("dQw4w9WgXcQ", "hqdefault"),
            "https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg"
        );
        assert_eq!(
            thumb_url("dQw4w9WgXcQ", "maxresdefault"),
            "https://i.ytimg.com/vi/dQw4w9WgXcQ/maxresdefault.jpg"
        );
    }

    #[test]
    fn oembed_url_encodes_the_canonical_url() {
        let canonical = canonical_url("dQw4w9WgXcQ");
        let u = url::Url::parse_with_params(
            "https://www.youtube.com/oembed",
            &[("url", canonical.as_str()), ("format", "json")],
        )
        .unwrap();
        assert_eq!(
            u.as_str(),
            "https://www.youtube.com/oembed?url=https%3A%2F%2Fwww.youtube.com%2Fwatch%3Fv%3DdQw4w9WgXcQ&format=json"
        );
    }

    #[test]
    fn iso8601_durations() {
        assert_eq!(parse_iso8601_duration("PT4M13S"), Some(253.0));
        assert_eq!(parse_iso8601_duration("PT1H"), Some(3600.0));
        assert_eq!(parse_iso8601_duration("PT0S"), Some(0.0));
        assert_eq!(parse_iso8601_duration("P1DT2H3M4S"), Some(93_784.0));
        assert_eq!(parse_iso8601_duration("PT1M30.5S"), Some(90.5));
        assert_eq!(parse_iso8601_duration("P1W"), Some(604_800.0));
        assert_eq!(parse_iso8601_duration("  PT2M  "), Some(120.0));
    }

    #[test]
    fn iso8601_rejects_garbage() {
        assert_eq!(parse_iso8601_duration(""), None);
        assert_eq!(parse_iso8601_duration("4M13S"), None); // P 없음
        assert_eq!(parse_iso8601_duration("P"), None);
        assert_eq!(parse_iso8601_duration("PT"), None);
        assert_eq!(parse_iso8601_duration("PTS"), None); // 숫자 없음
        assert_eq!(parse_iso8601_duration("PT5"), None); // 지정자 없음
        assert_eq!(parse_iso8601_duration("P1Y"), None); // 달력 의존 — 지어내지 않는다
        assert_eq!(parse_iso8601_duration("P1M"), None); // 날짜부 M도 마찬가지
        assert_eq!(parse_iso8601_duration("PT1X"), None);
    }

    #[test]
    fn guess_kind_follows_category_10() {
        // T11d — 카테고리 10(Music)만 audio다
        let mut m = YoutubeMeta::default();
        assert_eq!(m.guess_kind(), Kind::Video, "카테고리 미상이면 video");
        m.category_id = Some("10".into());
        assert_eq!(m.guess_kind(), Kind::Audio);
        m.category_id = Some("24".into());
        assert_eq!(m.guess_kind(), Kind::Video);
    }

    #[test]
    fn ytdlp_arguments_match_spec() {
        assert_eq!(section_arg(30), "*0-30");
        assert_eq!(section_arg(0), "*0-1", "0초 구간은 뜻이 없다");
        assert_eq!(format_selector(Kind::Audio), "ba");
        assert_eq!(
            format_selector(Kind::Video),
            "bv*[height<=720]+ba/b[height<=720]"
        );
    }

    #[test]
    fn bot_check_detection() {
        assert!(looks_like_bot_check(
            "ERROR: [youtube] xyz: Sign in to confirm you're not a bot. Use --cookies-from-browser"
        ));
        assert!(looks_like_bot_check("ERROR: This video is age-restricted"));
        // 쿠키를 붙여도 달라지지 않는 실패는 재시도하지 않는다
        assert!(!looks_like_bot_check(
            "ERROR: [youtube] xyz: Video unavailable"
        ));
        assert!(!looks_like_bot_check(
            "ERROR: Requested format is not available"
        ));
        assert!(!looks_like_bot_check(""));
    }

    #[test]
    fn cached_clip_ignores_partials_and_other_kinds() {
        let td = TempDir::new("cache");
        touch(&td.0, "abc123.video.mp4.part", b"partial");
        touch(&td.0, "abc123.audio.m4a", b"audio-bytes");
        touch(&td.0, "other.video.mp4", b"nope");
        assert!(cached_clip(&td.0, "abc123", Kind::Video).is_none());
        assert_eq!(
            cached_clip(&td.0, "abc123", Kind::Audio).unwrap(),
            td.0.join("abc123.audio.m4a")
        );
    }

    #[test]
    fn cached_clip_skips_empty_files() {
        let td = TempDir::new("empty");
        touch(&td.0, "abc123.video.mp4", b"");
        assert!(cached_clip(&td.0, "abc123", Kind::Video).is_none());
    }

    /// T11j — 캐시에 클립이 있으면 yt-dlp를 부르지 않는다.
    /// yt-dlp가 설치되지 않은 CI에서도 통과해야 하므로 이 테스트가 그 사실을 함께 증명한다.
    #[tokio::test]
    async fn download_clip_reuses_cached_file() {
        let td = TempDir::new("t11j");
        let want = touch(&td.0, "dQw4w9WgXcQ.video.mp4", b"cached clip bytes");
        let got = download_clip("dQw4w9WgXcQ", Kind::Video, &td.0, 30)
            .await
            .unwrap()
            .expect("캐시 적중이어야 한다");
        assert_eq!(got.path, want);
        assert_eq!(got.kind, Kind::Video);
    }

    #[tokio::test]
    async fn download_clip_rejects_bad_id_silently() {
        let td = TempDir::new("badid");
        let got = download_clip("../etc/passwd", Kind::Video, &td.0, 30)
            .await
            .unwrap();
        assert!(got.is_none(), "에러가 아니라 None이다");
    }

    #[tokio::test]
    async fn resolve_rejects_bad_id() {
        // 단계 1만 입력을 거부한다 (§9.3)
        assert!(resolve("not a video id", None, 5).await.is_err());
    }

    // ---- 네트워크 (기본 경로에서는 돌지 않는다) ----

    fn e2e_enabled() -> bool {
        std::env::var("SOUL_E2E").map(|v| v == "1").unwrap_or(false)
    }

    #[tokio::test]
    #[ignore = "네트워크. SOUL_E2E=1 일 때만"]
    async fn e2e_resolve_public_video() {
        if !e2e_enabled() {
            return;
        }
        let m = resolve("dQw4w9WgXcQ", None, 20).await.unwrap();
        assert_eq!(m.video_id, "dQw4w9WgXcQ");
        assert!(m.canonical_url.ends_with("dQw4w9WgXcQ"));
        assert!(m.thumbnail_url.contains("i.ytimg.com"));
        assert!(m.title.is_some(), "oEmbed가 제목을 줘야 한다");
    }

    #[tokio::test]
    #[ignore = "네트워크. SOUL_E2E=1 일 때만"]
    async fn e2e_fetch_thumbnail() {
        if !e2e_enabled() {
            return;
        }
        let bytes = fetch_thumbnail(&thumb_url("dQw4w9WgXcQ", "hqdefault"), 20)
            .await
            .unwrap();
        assert!(bytes.len() > 1000);
        assert_eq!(&bytes[..2], &[0xFF, 0xD8], "JPEG SOI");
    }
}
