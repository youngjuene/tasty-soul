//! 검색 제공자 (§11.1의 `web_search` · `web_fetch`).
//!
//! **모든 호출의 `query`와 방문한 URL을 빠짐없이 호출자에게 돌려준다.**
//! `context.queries`·`context.sources`가 §D6 프라이버시 고지의 근거이므로
//! 여기서 흘리면 고지가 거짓이 된다 (T47).
//!
//! 명세는 제공자를 지정하지 않았다 — `docs/OPEN-DECISIONS.md` #14 참조.
//!
//! ## 구현 메모
//!
//! - DuckDuckGo(기본값)는 결과 링크를 `//duckduckgo.com/l/?uddg=...` 리다이렉트로 감싼다.
//!   **그대로 두면 `context.sources`에 사용자가 확인할 수 없는 URL이 남는다.** 반드시 디코딩해
//!   실제 목적지를 기록한다. 목적지를 못 뽑으면 그 결과를 **버린다** — 근거로 쓸 수 없는 URL을
//!   근거처럼 적어두는 편이 빠뜨리는 것보다 나쁘다.
//! - `web_fetch`는 응답을 `MAX_FETCH_BYTES`에서 끊는다. 상한이 없으면 스트리밍 응답 하나가
//!   메모리를 통째로 먹는다.
//! - 본문은 **문자 수** 4000자로 자른다 (§11.1). 바이트로 자르면 한글이 깨진다.

use scraper::{Html, Node, Selector};
use soul_core::config::SearchConfig;
use soul_core::error::{Result, SoulError};
use std::time::Duration;

/// 평범한 브라우저 UA. HTML 엔드포인트는 낯선 UA에 빈 페이지를 준다.
const BROWSER_UA: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) \
     Chrome/124.0.0.0 Safari/537.36";

/// 검색 호출 상한. §11.1의 `agent_timeout_critique_secs`(90초) 안에 3회가 들어가야 한다.
const SEARCH_TIMEOUT_SECS: u64 = 20;

/// 본문 최대 4000자 (§11.1).
pub const MAX_TEXT_CHARS: usize = 4000;

/// 응답 본문 상한. 넘으면 자른다.
pub const MAX_FETCH_BYTES: usize = 5 * 1024 * 1024;

/// 스니펫 상한. 에이전트 컨텍스트를 한 결과가 독차지하지 못하게 한다.
const MAX_SNIPPET_CHARS: usize = 500;

/// 텍스트로 만들 때 통째로 버리는 요소 (§11.1). `head`는 `<title>`을 따로 뽑으므로 제외한다.
const SKIP_ELEMENTS: [&str; 9] = [
    "script", "style", "noscript", "nav", "footer", "head", "template", "svg", "iframe",
];

/// 앞에 줄바꿈을 넣는 요소. 없으면 문단이 한 줄로 뭉쳐 모델이 경계를 못 읽는다.
const BLOCK_ELEMENTS: [&str; 24] = [
    "p",
    "div",
    "br",
    "hr",
    "li",
    "ul",
    "ol",
    "tr",
    "td",
    "th",
    "table",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "section",
    "article",
    "header",
    "aside",
    "blockquote",
    "pre",
    "figcaption",
];

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

fn net_err(e: reqwest::Error) -> SoulError {
    SoulError::invalid(format!("네트워크: {e}"))
}

fn http(timeout_secs: u64) -> Result<reqwest::Client> {
    let mut b = reqwest::Client::builder().user_agent(BROWSER_UA);
    if timeout_secs > 0 {
        b = b.timeout(Duration::from_secs(timeout_secs));
    }
    b.build()
        .map_err(|e| SoulError::invalid(format!("HTTP 클라이언트 생성 실패: {e}")))
}

/// `(query) -> [{title, url, snippet}]`
pub async fn web_search(
    cfg: &SearchConfig,
    api_key: Option<&str>,
    query: &str,
) -> Result<Vec<SearchHit>> {
    let q = query.trim();
    if q.is_empty() {
        return Err(SoulError::invalid("검색어가 비었습니다"));
    }
    // 0은 "무제한"이 아니라 설정 실수다. 기본값으로 되돌린다.
    let max = if cfg.max_results == 0 {
        8
    } else {
        cfg.max_results.min(50)
    };

    // 인자로 받은 키(키체인)가 우선, 없으면 config.toml 값.
    let key: Option<&str> = api_key
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .or_else(|| Some(cfg.api_key.trim()).filter(|k| !k.is_empty()));

    let provider = cfg.provider.trim().to_ascii_lowercase();
    let mut hits = match provider.as_str() {
        "" | "duckduckgo" | "ddg" => duckduckgo(q, max).await?,
        "brave" => {
            let key = key.ok_or_else(|| {
                SoulError::config("brave 검색은 API 키가 필요합니다 ([search] api_key 또는 키체인)")
            })?;
            brave(q, key, max).await?
        }
        "tavily" => {
            let key = key.ok_or_else(|| {
                SoulError::config(
                    "tavily 검색은 API 키가 필요합니다 ([search] api_key 또는 키체인)",
                )
            })?;
            tavily(q, key, max).await?
        }
        other => {
            return Err(SoulError::config(format!(
                "알 수 없는 검색 제공자 {other:?} — duckduckgo · brave · tavily 중 하나"
            )))
        }
    };
    hits.truncate(max);
    Ok(hits)
}

// ───────────────────────────────────────────────────────────── DuckDuckGo

/// 키 없이 동작하는 기본 제공자 (OPEN-DECISIONS #14). HTML 엔드포인트를 긁는다.
async fn duckduckgo(query: &str, max: usize) -> Result<Vec<SearchHit>> {
    let client = http(SEARCH_TIMEOUT_SECS)?;
    // reqwest의 `form` 기능은 켜져 있지 않으므로 본문을 직접 만든다.
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("q", query)
        .finish();
    let resp = client
        .post("https://html.duckduckgo.com/html/")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("accept", "text/html,application/xhtml+xml")
        .header("accept-language", "en-US,en;q=0.9")
        .body(body)
        .send()
        .await
        .map_err(net_err)?;
    let status = resp.status();
    let html = resp.text().await.map_err(net_err)?;
    if !status.is_success() {
        return Err(SoulError::invalid(format!("duckduckgo 응답 {status}")));
    }
    Ok(parse_ddg_html(&html, max))
}

/// DDG HTML → 검색 결과. 네트워크와 분리해 두어야 파싱을 고정 입력으로 시험할 수 있다.
pub fn parse_ddg_html(html: &str, max: usize) -> Vec<SearchHit> {
    let (result_sel, title_sel, snippet_sel, any_a) = match (
        Selector::parse("div.result, div.web-result, .result, .web-result").ok(),
        Selector::parse("a.result__a").ok(),
        Selector::parse(".result__snippet").ok(),
        Selector::parse("h2 a, a").ok(),
    ) {
        (Some(a), Some(b), Some(c), Some(d)) => (a, b, c, d),
        _ => return Vec::new(),
    };

    let doc = Html::parse_document(html);
    let mut out: Vec<SearchHit> = Vec::new();

    for el in doc.select(&result_sel) {
        if out.len() >= max {
            break;
        }
        // 광고는 근거가 아니다
        let classes = el.value().attr("class").unwrap_or("");
        if classes
            .split_whitespace()
            .any(|c| c.starts_with("result--ad"))
        {
            continue;
        }
        let anchor = match el
            .select(&title_sel)
            .next()
            .or_else(|| el.select(&any_a).next())
        {
            Some(a) => a,
            None => continue,
        };
        let href = anchor.value().attr("href").unwrap_or("");
        let url = match decode_ddg_url(href) {
            Some(u) => u,
            None => continue,
        };
        let title = normalize_line(&anchor.text().collect::<String>());
        if title.is_empty() {
            continue;
        }
        if out.iter().any(|h| h.url == url) {
            continue;
        }
        let snippet = el
            .select(&snippet_sel)
            .next()
            .map(|s| normalize_line(&s.text().collect::<String>()))
            .unwrap_or_default();
        out.push(SearchHit {
            title,
            url,
            snippet: truncate_chars(&snippet, MAX_SNIPPET_CHARS),
        });
    }
    out
}

/// DDG 리다이렉트(`//duckduckgo.com/l/?uddg=...`)를 실제 목적지로 되돌린다.
///
/// 목적지를 못 뽑거나 http(s)가 아니면 `None`이다 — `context.sources`에는
/// **사용자가 열어서 확인할 수 있는 URL만** 남긴다 (§11.1 · §D6).
pub fn decode_ddg_url(href: &str) -> Option<String> {
    let h = href.trim();
    if h.is_empty() {
        return None;
    }
    // 프로토콜 상대(`//host/...`)와 루트 상대(`/l/?...`) 둘 다 온다
    let abs = if let Some(rest) = h.strip_prefix("//") {
        format!("https://{rest}")
    } else if h.starts_with('/') {
        format!("https://duckduckgo.com{h}")
    } else {
        h.to_string()
    };
    let url = url::Url::parse(&abs).ok()?;

    let is_ddg = url
        .host_str()
        .map(|host| host == "duckduckgo.com" || host.ends_with(".duckduckgo.com"))
        .unwrap_or(false);
    if is_ddg {
        // 광고 클릭 추적(`/y.js`)은 근거가 아니다
        if !url.path().starts_with("/l/") {
            return None;
        }
        let target = url
            .query_pairs()
            .find(|(k, _)| k == "uddg")
            .map(|(_, v)| v.into_owned())?;
        let parsed = url::Url::parse(target.trim()).ok()?;
        return http_only(parsed);
    }
    http_only(url)
}

fn http_only(u: url::Url) -> Option<String> {
    match u.scheme() {
        "http" | "https" => Some(u.to_string()),
        _ => None,
    }
}

// ───────────────────────────────────────────────────────────── URL 정규화

/// 값이 무엇이든 **페이지 내용을 바꾸지 않는** 추적 파라미터.
/// 여기에 없는 키는 지우지 않는다 — `?v=...`처럼 페이지를 정하는 쿼리를 지우면
/// 서로 다른 페이지가 한 건으로 뭉친다.
const TRACKING_PARAMS: [&str; 14] = [
    "fbclid", "gclid", "dclid", "gbraid", "wbraid", "msclkid", "yclid", "mc_cid", "mc_eid",
    "igshid", "ref_src", "ref_url", "_ga", "_gl",
];

fn is_tracking_param(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    k.starts_with("utm_") || TRACKING_PARAMS.contains(&k.as_str())
}

/// 같은 페이지를 가리키는 URL들을 **한 문자열**로 모은다 (§18-7).
///
/// 프래그먼트 제거 · 호스트 소문자화 · 끝 슬래시 통일 · 추적 쿼리 제거.
/// 결과가 같으면 같은 페이지다.
///
/// **왜 여기에 있나:** `context.sources`의 개수가 곧 `grounded`다 (§6.4). 문자열이
/// 다르다는 이유만으로 같은 페이지를 두 번 세면 화면 2.1은 "근거 2건"이라고 말하지만
/// 사용자가 링크를 눌러 확인하면 같은 글이 두 번 열린다. 근거를 세는 단위를 URL 문자열이
/// 아니라 **페이지**로 못 박는 것이 이 함수의 전부다.
///
/// 파싱이 안 되는 문자열은 다듬기만 해서 그대로 돌려준다. 빈 문자열로 뭉개면
/// 서로 다른 URL이 전부 같은 근거가 되어 버린다 — 못 세는 편이 잘못 세는 편보다 낫다.
pub fn canonical_url(raw: &str) -> String {
    let trimmed = raw.trim();
    let mut u = match url::Url::parse(trimmed) {
        Ok(u) => u,
        Err(_) => return trimmed.to_string(),
    };

    // 프래그먼트는 서버에 가지도 않는다. `#intro`와 `#credits`는 같은 문서다.
    u.set_fragment(None);

    // 추적 쿼리만 걷어낸다. 지울 것이 없으면 원래 인코딩을 그대로 둔다 —
    // 재직렬화는 `?a`를 `?a=`로 바꾸는 등 쓸데없이 URL을 흔든다.
    let pairs: Vec<(String, String)> = u
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let kept: Vec<&(String, String)> = pairs
        .iter()
        .filter(|(k, _)| !is_tracking_param(k))
        .collect();
    if kept.len() != pairs.len() {
        if kept.is_empty() {
            u.set_query(None);
        } else {
            let mut ser = url::form_urlencoded::Serializer::new(String::new());
            for (k, v) in kept {
                ser.append_pair(k, v);
            }
            u.set_query(Some(&ser.finish()));
        }
    }

    // `/a/`와 `/a`는 같은 글이다. 루트(`/`)는 지우면 URL이 깨지므로 남긴다.
    // (호스트·스킴 소문자화는 `url` 크레이트가 http(s)에 대해 이미 해 준다.)
    let path = u.path().to_string();
    if path.len() > 1 && path.ends_with('/') {
        let stripped = path.trim_end_matches('/');
        u.set_path(if stripped.is_empty() { "/" } else { stripped });
    }

    u.to_string()
}

// ───────────────────────────────────────────────────────────────── Brave

async fn brave(query: &str, api_key: &str, max: usize) -> Result<Vec<SearchHit>> {
    let client = http(SEARCH_TIMEOUT_SECS)?;
    let count = max.clamp(1, 20).to_string();
    let url = url::Url::parse_with_params(
        "https://api.search.brave.com/res/v1/web/search",
        &[("q", query), ("count", count.as_str())],
    )
    .map_err(|e| SoulError::invalid(format!("brave URL: {e}")))?;
    let resp = client
        .get(url.as_str())
        .header("accept", "application/json")
        .header("x-subscription-token", api_key)
        .send()
        .await
        .map_err(net_err)?;
    let status = resp.status();
    let body = resp.text().await.map_err(net_err)?;
    if !status.is_success() {
        return Err(SoulError::invalid(format!("brave 응답 {status}")));
    }
    let v: serde_json::Value = serde_json::from_str(&body)?;
    Ok(hits_from_json(
        v.get("web").and_then(|w| w.get("results")),
        "description",
        max,
    ))
}

// ──────────────────────────────────────────────────────────────── Tavily

async fn tavily(query: &str, api_key: &str, max: usize) -> Result<Vec<SearchHit>> {
    let client = http(SEARCH_TIMEOUT_SECS)?;
    let body = serde_json::json!({
        "api_key": api_key,
        "query": query,
        "max_results": max,
        "search_depth": "basic",
    });
    let resp = client
        .post("https://api.tavily.com/search")
        .header("authorization", format!("Bearer {api_key}"))
        .json(&body)
        .send()
        .await
        .map_err(net_err)?;
    let status = resp.status();
    let text = resp.text().await.map_err(net_err)?;
    if !status.is_success() {
        return Err(SoulError::invalid(format!("tavily 응답 {status}")));
    }
    let v: serde_json::Value = serde_json::from_str(&text)?;
    Ok(hits_from_json(v.get("results"), "content", max))
}

/// `[{title, url, <snippet_field>}]` 모양의 JSON 배열을 결과로 옮긴다.
fn hits_from_json(
    arr: Option<&serde_json::Value>,
    snippet_field: &str,
    max: usize,
) -> Vec<SearchHit> {
    let arr = match arr.and_then(|a| a.as_array()) {
        Some(a) => a,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for item in arr {
        if out.len() >= max {
            break;
        }
        let url = match item.get("url").and_then(|u| u.as_str()) {
            Some(u) if !u.trim().is_empty() => u.trim().to_string(),
            _ => continue,
        };
        let title = item
            .get("title")
            .and_then(|t| t.as_str())
            .map(strip_html)
            .unwrap_or_default();
        let snippet = item
            .get(snippet_field)
            .and_then(|s| s.as_str())
            .map(strip_html)
            .unwrap_or_default();
        if out.iter().any(|h: &SearchHit| h.url == url) {
            continue;
        }
        out.push(SearchHit {
            title,
            url,
            snippet: truncate_chars(&snippet, MAX_SNIPPET_CHARS),
        });
    }
    out
}

/// 제공자 응답에 섞여 오는 `<strong>` 같은 강조 태그를 벗긴다.
fn strip_html(s: &str) -> String {
    if !s.contains('<') {
        return normalize_line(s);
    }
    let frag = Html::parse_fragment(s);
    normalize_line(&document_text(&frag))
}

// ─────────────────────────────────────────────────────────────── web_fetch

#[derive(Clone, Debug)]
pub struct FetchedPage {
    pub title: String,
    /// **최대 4000자** (§11.1).
    pub text: String,
    /// 리다이렉트를 다 따라간 뒤의 **최종 URL**. 요청한 URL과 다를 수 있다.
    ///
    /// 근거 중복 판정은 요청 URL이 아니라 이 값으로 해야 한다 (§18-7).
    /// 단축 링크·추적 링크·`?amp` 변형은 문자열이 다 다르지만 결국 같은 글에 도착한다.
    pub final_url: String,
}

/// `(url) -> {title, text, final_url}` — 본문 최대 4000자.
///
/// 본문이 비어 있어도 **성공으로 돌려준다.** 근거로 셀지 말지는 호출자(§11.1의 비평 루프)가
/// 정한다 — 여기서 에러로 바꾸면 "본문 없음"과 "못 가져옴"이 한 덩어리가 되어,
/// 모델이 무엇을 다시 해야 하는지 알 수 없다.
pub async fn web_fetch(url: &str, timeout_secs: u64) -> Result<FetchedPage> {
    let target = url::Url::parse(url.trim())
        .map_err(|e| SoulError::invalid(format!("URL 파싱 실패 ({url}): {e}")))?;
    if !matches!(target.scheme(), "http" | "https") {
        return Err(SoulError::invalid(format!(
            "http(s)만 가져옵니다: {}",
            target.scheme()
        )));
    }

    let client = http(if timeout_secs == 0 {
        SEARCH_TIMEOUT_SECS
    } else {
        timeout_secs
    })?;
    let mut resp = client
        .get(target.as_str())
        .header(
            "accept",
            "text/html,application/xhtml+xml,text/plain;q=0.9,*/*;q=0.5",
        )
        .header("accept-language", "en-US,en;q=0.9")
        .send()
        .await
        .map_err(net_err)?;
    // 리다이렉트를 다 따라간 뒤의 주소. `resp`를 소비하기 전에 뽑아 둔다.
    let final_url = resp.url().to_string();
    let status = resp.status();
    if !status.is_success() {
        return Err(SoulError::invalid(format!("{final_url} → HTTP {status}")));
    }
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();

    // 상한까지만 읽는다. 넘는 부분은 버린다.
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(net_err)? {
        let room = MAX_FETCH_BYTES.saturating_sub(buf.len());
        if room == 0 {
            break;
        }
        if chunk.len() > room {
            buf.extend_from_slice(&chunk[..room]);
            break;
        }
        buf.extend_from_slice(&chunk);
    }
    // 상한에서 잘리면 UTF-8 경계가 깨질 수 있다. lossy가 정답이다.
    let body = String::from_utf8_lossy(&buf).into_owned();

    let looks_html = content_type.contains("html") || content_type.contains("xml") || {
        let head = body.trim_start().to_ascii_lowercase();
        head.starts_with("<!doctype html") || head.starts_with("<html")
    };

    if looks_html {
        // 제목이 없을 때의 대체 제목도 **최종 URL**이다. 리다이렉트로 온 페이지에
        // 요청 URL을 제목으로 달아두면 사용자가 다른 글로 읽는다.
        let (title, text) = html_to_text(&body, &final_url);
        Ok(FetchedPage {
            title,
            text,
            final_url,
        })
    } else {
        // 비HTML은 그대로 텍스트로 본다 (§11.1)
        Ok(FetchedPage {
            title: final_url.clone(),
            text: truncate_chars(&normalize_text(&body), MAX_TEXT_CHARS),
            final_url,
        })
    }
}

/// HTML → `(title, text)`. script·style·nav·footer를 버리고 공백을 정규화한 뒤 4000자로 자른다.
///
/// `Html`은 `Send`가 아니므로 **동기 함수로 분리해 둔다.** async 본문에서 살아 있으면
/// 호출자의 future가 `Send`를 잃는다.
pub fn html_to_text(html: &str, fallback_title: &str) -> (String, String) {
    let doc = Html::parse_document(html);
    let title = document_title(&doc).unwrap_or_else(|| fallback_title.to_string());
    let text = truncate_chars(&normalize_text(&document_text(&doc)), MAX_TEXT_CHARS);
    (title, text)
}

fn document_title(doc: &Html) -> Option<String> {
    let sel = Selector::parse("title").ok()?;
    let el = doc.select(&sel).next()?;
    let t = normalize_line(&el.text().collect::<String>());
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

/// 전위 순회. `SKIP_ELEMENTS`는 자식까지 통째로 건너뛴다.
fn document_text(doc: &Html) -> String {
    let mut out = String::new();
    // 문서 순서를 지키려면 역순으로 넣고 pop 한다
    let mut stack: Vec<_> = doc.tree.root().children().rev().collect();
    while let Some(node) = stack.pop() {
        match node.value() {
            Node::Element(el) => {
                let name = el.name();
                if SKIP_ELEMENTS.contains(&name) {
                    continue;
                }
                if BLOCK_ELEMENTS.contains(&name) {
                    out.push('\n');
                }
                stack.extend(node.children().rev());
            }
            Node::Text(t) => out.push_str(t),
            _ => {}
        }
    }
    out
}

/// 제목·스니펫용 한 줄 정규화. 줄바꿈까지 공백 하나로 만든다.
///
/// 본문과 달리 제목과 스니펫은 **한 줄이어야 한다** — 목록 UI와 프롬프트에 그대로 들어간다.
fn normalize_line(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 본문 공백 정규화. 연속 공백은 하나로, 줄바꿈은 최대 두 개까지 남긴다.
fn normalize_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut pending_nl = 0usize;
    let mut pending_sp = false;
    for ch in raw.chars() {
        if ch == '\n' || ch == '\r' {
            pending_nl += 1;
            pending_sp = false;
            continue;
        }
        if ch.is_whitespace() {
            pending_sp = true;
            continue;
        }
        if !out.is_empty() {
            if pending_nl > 0 {
                for _ in 0..pending_nl.min(2) {
                    out.push('\n');
                }
            } else if pending_sp {
                out.push(' ');
            }
        }
        pending_nl = 0;
        pending_sp = false;
        out.push(ch);
    }
    out
}

/// **문자** 기준 절단. 바이트로 자르면 한글이 깨진다.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DDG_FIXTURE: &str = r##"
<html><body>
<div class="result results_links results_links_deep web-result">
  <div class="links_main links_deep result__body">
    <h2 class="result__title">
      <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa%3Fb%3D1&amp;rut=deadbeef">첫 <b>결과</b></a>
    </h2>
    <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa">본문   요약
    두 번째 줄</a>
  </div>
</div>
<div class="result results_links result--ad">
  <h2 class="result__title"><a class="result__a" href="//duckduckgo.com/y.js?ad=1">광고</a></h2>
</div>
<div class="web-result">
  <h2><a class="result__a" href="https://direct.example.org/page">직접 링크</a></h2>
  <div class="result__snippet">스니펫 둘</div>
</div>
</body></html>
"##;

    #[test]
    fn ddg_parsing_extracts_title_url_snippet() {
        let hits = parse_ddg_html(DDG_FIXTURE, 10);
        assert_eq!(hits.len(), 2, "광고 결과는 빠진다");
        assert_eq!(hits[0].title, "첫 결과");
        assert_eq!(
            hits[0].url, "https://example.com/a?b=1",
            "리다이렉트가 아니라 실제 URL이어야 한다 (§D6)"
        );
        assert_eq!(
            hits[0].snippet, "본문 요약 두 번째 줄",
            "스니펫은 한 줄이다"
        );
        assert_eq!(hits[1].url, "https://direct.example.org/page");
        assert_eq!(hits[1].snippet, "스니펫 둘");
    }

    #[test]
    fn ddg_parsing_respects_max() {
        let hits = parse_ddg_html(DDG_FIXTURE, 1);
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn ddg_redirect_is_decoded() {
        assert_eq!(
            decode_ddg_url(
                "//duckduckgo.com/l/?uddg=https%3A%2F%2Fen.wikipedia.org%2Fwiki%2FKimchi&rut=x"
            )
            .unwrap(),
            "https://en.wikipedia.org/wiki/Kimchi"
        );
        assert_eq!(
            decode_ddg_url("/l/?uddg=http%3A%2F%2Fexample.com%2F%3Fq%3D%ED%95%9C%EA%B8%80")
                .unwrap(),
            "http://example.com/?q=%ED%95%9C%EA%B8%80"
        );
    }

    #[test]
    fn ddg_non_redirect_urls_pass_through() {
        assert_eq!(
            decode_ddg_url("https://example.com/x").unwrap(),
            "https://example.com/x"
        );
        assert_eq!(
            decode_ddg_url("  https://example.com/y  ").unwrap(),
            "https://example.com/y"
        );
    }

    #[test]
    fn ddg_unusable_urls_are_dropped() {
        // 확인할 수 없는 URL은 근거로 기록하지 않는다
        assert!(decode_ddg_url("").is_none());
        assert!(decode_ddg_url("//duckduckgo.com/y.js?ad_provider=x").is_none());
        assert!(decode_ddg_url("//duckduckgo.com/l/?rut=nouddg").is_none());
        assert!(decode_ddg_url("javascript:alert(1)").is_none());
        assert!(decode_ddg_url("//duckduckgo.com/l/?uddg=javascript%3Aalert(1)").is_none());
        assert!(decode_ddg_url("not a url").is_none());
    }

    // ---- URL 정규화 (§18-7 — 근거를 페이지 단위로 센다) ----

    #[test]
    fn canonical_url_drops_the_fragment() {
        assert_eq!(
            canonical_url("https://example.org/a#intro"),
            canonical_url("https://example.org/a#credits")
        );
        assert_eq!(
            canonical_url("https://example.org/a#intro"),
            "https://example.org/a"
        );
    }

    #[test]
    fn canonical_url_lowercases_the_host_and_unifies_the_trailing_slash() {
        assert_eq!(
            canonical_url("https://EXAMPLE.org/Article/"),
            canonical_url("https://example.org/Article")
        );
        // 경로의 대소문자는 서버가 구분할 수 있으므로 건드리지 않는다.
        assert_ne!(
            canonical_url("https://example.org/Article"),
            canonical_url("https://example.org/article")
        );
        // 루트의 슬래시는 남는다 — 지우면 URL이 깨진다.
        assert_eq!(
            canonical_url("https://example.org/"),
            "https://example.org/"
        );
        assert_eq!(canonical_url("https://example.org"), "https://example.org/");
    }

    #[test]
    fn canonical_url_drops_tracking_queries_only() {
        assert_eq!(
            canonical_url("https://example.org/a?utm_source=x&utm_medium=y&fbclid=z"),
            "https://example.org/a"
        );
        assert_eq!(
            canonical_url("https://example.org/watch?v=abc&gclid=zzz"),
            "https://example.org/watch?v=abc",
            "페이지를 정하는 쿼리는 남아야 한다"
        );
        // 지울 것이 없으면 원래 쿼리 문자열을 그대로 둔다.
        assert_eq!(
            canonical_url("https://example.org/a?b=1&a"),
            "https://example.org/a?b=1&a"
        );
    }

    #[test]
    fn canonical_url_keeps_unparsable_input_distinct() {
        // 못 세는 것보다 잘못 세는 것이 나쁘다 — 빈 문자열로 뭉개지 않는다.
        assert_eq!(canonical_url("  not a url  "), "not a url");
        assert_ne!(canonical_url("not a url"), canonical_url("also not a url"));
    }

    #[test]
    fn html_to_text_drops_script_style_nav_footer() {
        let html = r#"<html><head><title>  제목
        입니다 </title><style>body{color:red}</style></head>
        <body><nav>메뉴1 메뉴2</nav><p>본문 하나</p>
        <script>var x = "숨은 코드";</script>
        <p>본문 둘</p><footer>바닥글</footer></body></html>"#;
        let (title, text) = html_to_text(html, "fallback");
        assert_eq!(title, "제목 입니다");
        assert!(text.contains("본문 하나"));
        assert!(text.contains("본문 둘"));
        assert!(!text.contains("메뉴1"), "nav 제거");
        assert!(!text.contains("숨은 코드"), "script 제거");
        assert!(!text.contains("바닥글"), "footer 제거");
        assert!(!text.contains("color:red"), "style 제거");
        assert!(!text.contains("제목"), "title은 본문에 다시 넣지 않는다");
    }

    #[test]
    fn html_to_text_truncates_at_4000_chars() {
        // 한글로 채운다 — 바이트로 자르면 여기서 깨진다
        let long = "가".repeat(5000);
        let html = format!("<html><head><title>T</title></head><body><p>{long}</p></body></html>");
        let (title, text) = html_to_text(&html, "fallback");
        assert_eq!(title, "T");
        assert_eq!(text.chars().count(), MAX_TEXT_CHARS);
        assert!(text.chars().all(|c| c == '가'));
    }

    #[test]
    fn html_to_text_falls_back_to_given_title() {
        let (title, _) = html_to_text("<html><body><p>x</p></body></html>", "https://e.org/a");
        assert_eq!(title, "https://e.org/a");
    }

    #[test]
    fn block_elements_become_line_breaks() {
        let (_, text) = html_to_text("<body><p>하나</p><p>둘</p><div>셋</div></body>", "t");
        assert_eq!(text, "하나\n둘\n셋");
    }

    #[test]
    fn normalize_collapses_whitespace() {
        assert_eq!(normalize_text("  a \t  b \u{a0}c  "), "a b c");
        assert_eq!(normalize_text("a\n\n\n\n\nb"), "a\n\nb");
        assert_eq!(normalize_text("   "), "");
    }

    #[test]
    fn json_providers_map_fields() {
        let brave_body = serde_json::json!({
            "web": {"results": [
                {"title": "제목 <strong>강조</strong>", "url": "https://a.example/1", "description": "설명 <strong>x</strong>"},
                {"title": "둘", "url": "https://a.example/2", "description": "설명2"},
                {"title": "중복", "url": "https://a.example/2", "description": "무시"},
                {"title": "url 없음", "description": "무시"}
            ]}
        });
        let hits = hits_from_json(
            brave_body.get("web").and_then(|w| w.get("results")),
            "description",
            10,
        );
        assert_eq!(hits.len(), 2, "중복 URL과 url 없는 항목은 빠진다");
        assert_eq!(hits[0].title, "제목 강조");
        assert_eq!(hits[0].snippet, "설명 x");
        assert_eq!(hits[1].url, "https://a.example/2");
    }

    #[test]
    fn tavily_shape_uses_content_field() {
        let body = serde_json::json!({
            "results": [{"title": "t", "url": "https://x.example/", "content": "본문"}]
        });
        let hits = hits_from_json(body.get("results"), "content", 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].snippet, "본문");
    }

    #[tokio::test]
    async fn empty_query_is_rejected() {
        let cfg = SearchConfig::default();
        assert!(web_search(&cfg, None, "   ").await.is_err());
    }

    #[tokio::test]
    async fn unknown_provider_is_a_config_error() {
        let cfg = SearchConfig {
            provider: "google".into(),
            ..SearchConfig::default()
        };
        let err = web_search(&cfg, None, "김치").await.unwrap_err();
        assert!(matches!(err, SoulError::Config(_)), "실제: {err}");
    }

    #[tokio::test]
    async fn keyed_providers_require_a_key() {
        for p in ["brave", "tavily"] {
            let cfg = SearchConfig {
                provider: p.into(),
                api_key: String::new(),
                ..SearchConfig::default()
            };
            let err = web_search(&cfg, None, "김치").await.unwrap_err();
            assert!(matches!(err, SoulError::Config(_)), "{p}: {err}");
        }
    }

    /// `Html`은 `Send`가 아니다. async 본문에 새어 들어가면 호출자(에이전트 루프)가
    /// `tokio::spawn` 하지 못한다 — 컴파일 시점에 못 박아 둔다.
    #[test]
    fn futures_stay_send() {
        fn assert_send<T: Send>(_: T) {}
        let cfg = SearchConfig::default();
        assert_send(web_search(&cfg, None, "김치"));
        assert_send(web_fetch("https://example.com", 5));
    }

    #[tokio::test]
    async fn web_fetch_rejects_non_http_schemes() {
        // file:// 로 로컬 파일을 읽히지 않는다
        assert!(web_fetch("file:///etc/passwd", 5).await.is_err());
        assert!(web_fetch("not a url", 5).await.is_err());
    }

    // ---- 네트워크 (기본 경로에서는 돌지 않는다) ----

    fn e2e_enabled() -> bool {
        std::env::var("SOUL_E2E").map(|v| v == "1").unwrap_or(false)
    }

    #[tokio::test]
    #[ignore = "네트워크. SOUL_E2E=1 일 때만"]
    async fn e2e_duckduckgo_returns_hits() {
        if !e2e_enabled() {
            return;
        }
        let cfg = SearchConfig::default();
        let hits = web_search(&cfg, None, "kimchi fermentation").await.unwrap();
        assert!(!hits.is_empty());
        for h in &hits {
            assert!(h.url.starts_with("http"), "{}", h.url);
            assert!(
                !h.url.contains("duckduckgo.com/l/"),
                "리다이렉트가 남았다: {}",
                h.url
            );
        }
    }

    #[tokio::test]
    #[ignore = "네트워크. SOUL_E2E=1 일 때만"]
    async fn e2e_web_fetch_truncates() {
        if !e2e_enabled() {
            return;
        }
        let page = web_fetch("https://en.wikipedia.org/wiki/Kimchi", 30)
            .await
            .unwrap();
        assert!(page.text.chars().count() <= MAX_TEXT_CHARS);
        assert!(page.title.to_lowercase().contains("kimchi"));
        // 리다이렉트가 없어도 최종 URL은 채워져 있어야 한다 — 중복 판정의 기준이다.
        assert!(page.final_url.starts_with("https://"), "{}", page.final_url);
    }
}
