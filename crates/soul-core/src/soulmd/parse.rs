//! `SOUL.md` 파서 (§8.3).
//!
//! ## 구현 지시 — 이 규칙들을 완화하지 말 것
//!
//! 1. 마커는 **줄 단위 상태 기계**로 파싱한다. **정규식으로 전체 문서를 훑지 않는다.**
//! 2. 마커 줄은 앞뒤 공백을 허용하되 그 외 문자는 허용하지 않는다.
//!    (즉 `  <!-- soul:gen id=header -->  ` 는 유효, `x <!-- ... -->` 는 무효)
//! 3. 해시 정규화: 마커 사이 본문에 대해
//!    (a) CRLF→LF (b) **앞뒤의** 공백만 있는 줄 제거 (c) 각 줄의 뒤쪽 공백 제거.
//!    내부 빈 줄은 보존. 결과의 SHA-256 hex **앞 6자**.
//! 4. 저장 감지 시 각 `soul:neg` 블록의 실제 해시와 마커의 `hash`를 비교한다.
//! 5. **`soul:human` 본문은 파싱도 해싱도 하지 않고 통째로 건너뛴다.**
//! 6. 마커 짝이 맞지 않으면 파싱 실패로 처리하고 **파일을 쓰지 않는다.** 자동 복구하지 않는다 (T10).
//! 7. `soul:gen` 본문이 변경되었으면 "재빌드 시 덮어써집니다"를 알리고 그대로 진행한다.

use crate::error::{Result, SoulError};
use sha2::{Digest, Sha256};

/// 마커 종류 (§8.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    /// `<!-- soul:gen id=X -->` — 재빌드 시 덮어씀, 사용자 편집은 무시(경고).
    Gen,
    /// `<!-- soul:neg id=X rev=N hash=H -->` — 재생 결과로 채움, 편집 시 `profile_edit` 기록.
    Neg,
    /// `<!-- soul:human -->` — **건드리지 않음.** 관측 없음. 원격 전송 금지 (§D4).
    Human,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SoulBlock {
    pub kind: BlockKind,
    /// `soul:human`은 `None`. 그 외는 마커의 `id=`.
    pub id: Option<String>,
    /// `soul:neg`만 가진다.
    pub rev: Option<u32>,
    /// `soul:neg`만 가진다. 마커에 적힌 값(파일이 주장하는 해시).
    pub hash: Option<String>,
    /// 마커 사이의 원문 그대로. 정규화하지 않은 값이다.
    pub body: String,
    /// 여는 마커 줄 번호 (1-base). 에러 메시지용.
    pub line: usize,
}

impl SoulBlock {
    /// §8.3 규칙 3의 정규화를 적용한 본문의 실제 해시 앞 6자.
    /// `soul:human`에는 호출하지 않는다 (규칙 5).
    pub fn actual_hash(&self) -> String {
        block_hash(&self.body)
    }

    /// 마커가 주장하는 해시와 실제 해시가 다른가 (§8.3 규칙 4).
    pub fn is_dirty(&self) -> bool {
        match (&self.hash, self.kind) {
            (Some(h), BlockKind::Neg) => h != &self.actual_hash(),
            _ => false,
        }
    }

    /// 정규화된 본문 텍스트. `to_text`로 관측에 기록할 값이다.
    pub fn normalized_body(&self) -> String {
        normalize_for_hash(&self.body)
    }
}

/// 파싱된 문서. 블록 사이의 산문(제목·본문)도 순서대로 보존한다.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SoulDoc {
    /// 문서의 모든 조각. 렌더러가 아니라 **편집 저장 경로**가 이것을 쓴다.
    pub blocks: Vec<SoulBlock>,
    /// 파일 원문. `soul://md`(§19.3)와 §8.4 3단계의 `soul:human` 보관에 쓴다.
    pub raw: String,
}

impl SoulDoc {
    pub fn block(&self, id: &str) -> Option<&SoulBlock> {
        self.blocks.iter().find(|b| b.id.as_deref() == Some(id))
    }

    pub fn human_body(&self) -> Option<&str> {
        self.blocks
            .iter()
            .find(|b| b.kind == BlockKind::Human)
            .map(|b| b.body.as_str())
    }

    pub fn neg_blocks(&self) -> Vec<&SoulBlock> {
        self.blocks
            .iter()
            .filter(|b| b.kind == BlockKind::Neg)
            .collect()
    }

    /// 편집으로 내용이 달라진 `soul:neg` 블록들 (§8.4 4단계).
    pub fn dirty_neg_blocks(&self) -> Vec<&SoulBlock> {
        self.neg_blocks()
            .into_iter()
            .filter(|b| b.is_dirty())
            .collect()
    }
}

/// §8.3 규칙 3 — 해시 정규화.
///
/// (a) CRLF→LF (b) 앞뒤의 공백만 있는 줄 제거 (c) 각 줄의 뒤쪽 공백 제거.
/// **내부 빈 줄은 보존한다.**
pub fn normalize_for_hash(body: &str) -> String {
    // (a) CRLF→LF.
    let lf = body.replace("\r\n", "\n");

    // (c) 각 줄의 뒤쪽 공백 제거.
    // `str::lines`는 끝의 개행 하나를 줄로 세지 않으므로, 본문이 개행으로 끝나든
    // 말든 같은 결과가 나온다. 렌더러가 붙이는 마지막 개행이 해시를 바꾸지 않아야 한다.
    let lines: Vec<&str> = lf.lines().map(str::trim_end).collect();

    // (b) **앞뒤의** 공백만 있는 줄만 제거한다. 내부 빈 줄은 그대로 둔다.
    let start = lines.iter().position(|l| !l.is_empty());
    match start {
        // 전부 공백뿐인 본문은 빈 문자열로 정규화된다.
        None => String::new(),
        Some(start) => {
            // 위에서 비어 있지 않은 줄을 찾았으므로 `rposition`도 반드시 성공한다.
            let end = lines.iter().rposition(|l| !l.is_empty()).unwrap_or(start) + 1;
            lines[start..end].join("\n")
        }
    }
}

/// 정규화된 본문의 SHA-256 hex **앞 6자**.
pub fn block_hash(body: &str) -> String {
    let normalized = normalize_for_hash(body);
    let digest = Sha256::digest(normalized.as_bytes());
    let hex = hex::encode(digest);
    // sha256 hex는 항상 64자이므로 앞 6자 슬라이스는 불변식으로 보장된다.
    hex[..HASH_LEN].to_string()
}

/// 마커의 `hash=` 길이 (§8.3 규칙 3 — hex 앞 6자).
const HASH_LEN: usize = 6;

/// 한 줄을 해석한 결과.
enum Marker {
    /// 여는 마커. 파싱된 속성을 그대로 들고 있다.
    Open {
        kind: BlockKind,
        id: Option<String>,
        rev: Option<u32>,
        hash: Option<String>,
    },
    /// 닫는 마커.
    Close(BlockKind),
}

/// 여는 마커를 만난 뒤의 상태.
struct OpenState {
    kind: BlockKind,
    id: Option<String>,
    rev: Option<u32>,
    hash: Option<String>,
    /// 여는 마커 줄 번호 (1-base).
    line: usize,
    /// 본문 시작 바이트 오프셋 (여는 마커 줄 **다음** 바이트).
    body_start: usize,
}

fn parse_err(line: usize, reason: impl Into<String>) -> SoulError {
    SoulError::SoulParse {
        line,
        reason: reason.into(),
    }
}

/// `key=value` 한 토큰을 쪼갠다. 값이 비어 있으면 마커로 인정하지 않는다.
fn split_attr(tok: &str) -> Option<(&str, &str)> {
    let i = tok.find('=')?;
    let (k, rest) = tok.split_at(i);
    let v = &rest[1..];
    if k.is_empty() || v.is_empty() {
        None
    } else {
        Some((k, v))
    }
}

/// 한 줄이 soul 마커인지 판별한다 (§8.3 규칙 2).
///
/// 앞뒤 공백은 허용하고 **그 외 문자는 허용하지 않는다.** 마커가 아니면 `Ok(None)`이고
/// 그 줄은 본문(산문)이다. `soul:` 로 시작하지만 속성이 망가진 줄은 조용히 본문으로
/// 흘려보내지 않고 `SoulParse`로 세운다 — 자동 복구하지 않는다 (규칙 6).
fn classify_line(line: &str, lineno: usize) -> Result<Option<Marker>> {
    let t = line.trim();
    // HTML 주석 꼴이 아니면 본문이다. (`x <!-- soul:gen id=a -->` 도 여기서 걸러진다)
    if t.len() < 7 || !t.starts_with("<!--") || !t.ends_with("-->") {
        return Ok(None);
    }
    let inner = &t[4..t.len() - 3];

    let mut toks = inner.split_whitespace();
    let head = match toks.next() {
        Some(h) => h,
        // `<!-- -->` 같은 빈 주석.
        None => return Ok(None),
    };
    let rest: Vec<&str> = toks.collect();

    // 닫는 마커 3종 — 속성을 갖지 않는다.
    let close_kind = match head {
        "/soul:gen" => Some(BlockKind::Gen),
        "/soul:neg" => Some(BlockKind::Neg),
        "/soul:human" => Some(BlockKind::Human),
        _ => None,
    };
    if let Some(kind) = close_kind {
        if !rest.is_empty() {
            return Err(parse_err(
                lineno,
                format!("닫는 마커에 속성이 붙었습니다: {t}"),
            ));
        }
        return Ok(Some(Marker::Close(kind)));
    }

    match head {
        // `<!-- soul:gen id=X -->`
        "soul:gen" => {
            if rest.len() != 1 {
                return Err(parse_err(
                    lineno,
                    format!("soul:gen 마커는 id= 하나만 받습니다: {t}"),
                ));
            }
            match split_attr(rest[0]) {
                Some(("id", v)) => Ok(Some(Marker::Open {
                    kind: BlockKind::Gen,
                    id: Some(v.to_string()),
                    rev: None,
                    hash: None,
                })),
                _ => Err(parse_err(
                    lineno,
                    format!("soul:gen 마커에 id=가 없습니다: {t}"),
                )),
            }
        }
        // `<!-- soul:neg id=X rev=N hash=H -->`
        "soul:neg" => {
            if rest.len() != 3 {
                return Err(parse_err(
                    lineno,
                    format!("soul:neg 마커는 id=·rev=·hash= 세 속성이 필요합니다: {t}"),
                ));
            }
            let (mut id, mut rev, mut hash) = (None, None, None);
            for tok in &rest {
                let (k, v) = match split_attr(tok) {
                    Some(kv) => kv,
                    None => return Err(parse_err(lineno, format!("속성 형식이 아닙니다: {tok}"))),
                };
                let dup = match k {
                    "id" => id.replace(v.to_string()).is_some(),
                    "rev" => {
                        let n: u32 = v.parse().map_err(|_| {
                            parse_err(lineno, format!("rev=는 정수여야 합니다: {tok}"))
                        })?;
                        rev.replace(n).is_some()
                    }
                    "hash" => hash.replace(v.to_string()).is_some(),
                    _ => {
                        return Err(parse_err(lineno, format!("알 수 없는 속성입니다: {tok}")));
                    }
                };
                if dup {
                    return Err(parse_err(lineno, format!("속성이 중복됩니다: {k}=")));
                }
            }
            // 토큰이 3개이고 중복·미지 속성이 없었으므로 셋 다 채워져 있다.
            match (id, rev, hash) {
                (Some(id), Some(rev), Some(hash)) => Ok(Some(Marker::Open {
                    kind: BlockKind::Neg,
                    id: Some(id),
                    rev: Some(rev),
                    hash: Some(hash),
                })),
                _ => Err(parse_err(
                    lineno,
                    format!("soul:neg 마커에 id=·rev=·hash=가 모두 있어야 합니다: {t}"),
                )),
            }
        }
        // `<!-- soul:human -->`
        "soul:human" => {
            if !rest.is_empty() {
                return Err(parse_err(
                    lineno,
                    format!("soul:human 마커는 속성을 받지 않습니다: {t}"),
                ));
            }
            Ok(Some(Marker::Open {
                kind: BlockKind::Human,
                id: None,
                rev: None,
                hash: None,
            }))
        }
        // soul 마커가 아닌 평범한 주석은 본문이다.
        _ => Ok(None),
    }
}

fn kind_name(kind: BlockKind) -> &'static str {
    match kind {
        BlockKind::Gen => "soul:gen",
        BlockKind::Neg => "soul:neg",
        BlockKind::Human => "soul:human",
    }
}

/// 문서를 파싱한다. 마커 짝이 맞지 않으면 `SoulError::SoulParse` (§8.3 규칙 6).
pub fn parse(text: &str) -> Result<SoulDoc> {
    let mut blocks: Vec<SoulBlock> = Vec::new();
    let mut open: Option<OpenState> = None;
    // 본문을 원문 그대로 잘라내기 위한 바이트 커서.
    let mut cursor = 0usize;

    // 규칙 1 — 줄 단위 상태 기계. `split_inclusive`는 줄 끝 문자를 남기므로
    // 오프셋이 원문과 어긋나지 않는다.
    for (idx, raw_line) in text.split_inclusive('\n').enumerate() {
        let lineno = idx + 1;
        let line_start = cursor;
        cursor += raw_line.len();
        let line_end = cursor;

        // 규칙 5 — `soul:human` 본문은 파싱도 해싱도 하지 않고 통째로 건너뛴다.
        // 안에 있는 마커처럼 생긴 줄은 전부 본문이며, `<!-- /soul:human -->` 만 닫는다.
        if matches!(open.as_ref().map(|o| o.kind), Some(BlockKind::Human)) {
            let is_close = matches!(
                classify_line(raw_line, lineno),
                Ok(Some(Marker::Close(BlockKind::Human)))
            );
            if is_close {
                if let Some(o) = open.take() {
                    blocks.push(SoulBlock {
                        kind: BlockKind::Human,
                        id: None,
                        rev: None,
                        hash: None,
                        body: text[o.body_start..line_start].to_string(),
                        line: o.line,
                    });
                }
            }
            continue;
        }

        let marker = match classify_line(raw_line, lineno)? {
            Some(m) => m,
            None => continue,
        };

        match marker {
            Marker::Open {
                kind,
                id,
                rev,
                hash,
            } => {
                if let Some(o) = &open {
                    return Err(parse_err(
                        lineno,
                        format!(
                            "{} 블록({}행)이 닫히기 전에 {} 마커가 나왔습니다",
                            kind_name(o.kind),
                            o.line,
                            kind_name(kind)
                        ),
                    ));
                }
                open = Some(OpenState {
                    kind,
                    id,
                    rev,
                    hash,
                    line: lineno,
                    body_start: line_end,
                });
            }
            Marker::Close(kind) => match open.take() {
                None => {
                    return Err(parse_err(
                        lineno,
                        format!("여는 마커 없이 /{} 가 나왔습니다", kind_name(kind)),
                    ));
                }
                Some(o) if o.kind != kind => {
                    return Err(parse_err(
                        lineno,
                        format!(
                            "{} 블록({}행)을 /{} 로 닫으려 했습니다",
                            kind_name(o.kind),
                            o.line,
                            kind_name(kind)
                        ),
                    ));
                }
                Some(o) => {
                    blocks.push(SoulBlock {
                        kind: o.kind,
                        id: o.id,
                        rev: o.rev,
                        hash: o.hash,
                        body: text[o.body_start..line_start].to_string(),
                        line: o.line,
                    });
                }
            },
        }
    }

    if let Some(o) = open {
        return Err(parse_err(
            o.line,
            format!("{} 블록이 닫히지 않았습니다", kind_name(o.kind)),
        ));
    }

    Ok(SoulDoc {
        blocks,
        raw: text.to_string(),
    })
}

#[allow(dead_code)]
fn _unused(e: SoulError) -> SoulError {
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §8.2 템플릿 전문.
    const TEMPLATE: &str = r#"# SOUL

<!-- soul:gen id=header -->
기준 2026-08-13 · 관측 342 · 최초 2026-03-02
어긋남 0.31 · 해상도 0.58
<!-- /soul:gen -->

## 지금의 취향

<!-- soul:neg id=profile rev=17 hash=a3f91c -->
인공물보다 방치된 것을 고른다. 채도가 아니라 습도로
공간을 읽고, 사람이 방금 나간 자리에 반복해서 멈춘다.
<!-- /soul:neg -->

## 축

<!-- soul:gen id=axes -->
| 축 | 값 | 90일 변화 |
|---|---|---|
| chroma | 0.34 | — |
| luminance | 0.52 | +0.03 |
| density | 0.28 | −0.06 |
| grain | 0.71 | +0.14 |
| tempo | 0.19 | — |
| space | 0.66 | +0.02 |
| valence | 0.41 | −0.11 |
| intensity | 0.30 | +0.05 |
<!-- /soul:gen -->

## 어긋남

<!-- soul:gen id=divergence -->
- 일관성 0.62 · 정정 18건
- 대표 사례: 01J8XQB / 01J8XQC / 01J8XQD
<!-- /soul:gen -->

## 내가 쓴 것

<!-- soul:human -->

<!-- /soul:human -->
"#;

    fn parse_err_line(text: &str) -> usize {
        match parse(text) {
            Err(SoulError::SoulParse { line, .. }) => line,
            other => panic!("SoulParse를 기대했는데 {other:?} 가 나왔습니다"),
        }
    }

    // ---------- §8.2 템플릿 ----------

    #[test]
    fn template_yields_all_five_blocks_in_order() {
        let doc = parse(TEMPLATE).expect("템플릿은 파싱되어야 한다");
        let kinds: Vec<BlockKind> = doc.blocks.iter().map(|b| b.kind).collect();
        assert_eq!(
            kinds,
            vec![
                BlockKind::Gen,
                BlockKind::Neg,
                BlockKind::Gen,
                BlockKind::Gen,
                BlockKind::Human
            ]
        );
        let ids: Vec<Option<&str>> = doc.blocks.iter().map(|b| b.id.as_deref()).collect();
        assert_eq!(
            ids,
            vec![
                Some("header"),
                Some("profile"),
                Some("axes"),
                Some("divergence"),
                None
            ]
        );
    }

    #[test]
    fn template_neg_marker_attributes_are_parsed() {
        let doc = parse(TEMPLATE).unwrap();
        let profile = doc.block("profile").expect("profile 블록");
        assert_eq!(profile.kind, BlockKind::Neg);
        assert_eq!(profile.rev, Some(17));
        assert_eq!(profile.hash.as_deref(), Some("a3f91c"));
        // 여는 마커 줄 번호는 1-base.
        assert_eq!(profile.line, 10);
        assert_eq!(doc.block("header").unwrap().line, 3);
    }

    #[test]
    fn template_body_is_verbatim_between_markers() {
        let doc = parse(TEMPLATE).unwrap();
        assert_eq!(
            doc.block("header").unwrap().body,
            "기준 2026-08-13 · 관측 342 · 최초 2026-03-02\n어긋남 0.31 · 해상도 0.58\n"
        );
        // 표는 8축 전부가 본문에 그대로 남는다.
        let axes = &doc.block("axes").unwrap().body;
        assert!(axes.starts_with("| 축 | 값 | 90일 변화 |\n"));
        assert!(axes.contains("| intensity | 0.30 | +0.05 |\n"));
        // 마커 줄 자체는 본문에 포함되지 않는다.
        assert!(!axes.contains("soul:gen"));
    }

    #[test]
    fn raw_keeps_the_whole_document() {
        let doc = parse(TEMPLATE).unwrap();
        assert_eq!(doc.raw, TEMPLATE);
        // 블록 사이의 산문도 raw로 복원할 수 있다.
        assert!(doc.raw.contains("## 지금의 취향"));
    }

    #[test]
    fn human_block_body_is_kept_as_is() {
        let doc = parse(TEMPLATE).unwrap();
        // 템플릿의 human 본문은 빈 줄 하나다.
        assert_eq!(doc.human_body(), Some("\n"));
    }

    #[test]
    fn neg_blocks_and_dirty_detection() {
        let doc = parse(TEMPLATE).unwrap();
        assert_eq!(doc.neg_blocks().len(), 1);
        // 템플릿의 `a3f91c`는 예시값이므로 실제 해시와 다르다 → dirty.
        assert!(doc.block("profile").unwrap().is_dirty());
        // gen/human은 해시 비교 대상이 아니다 (규칙 4·5).
        assert!(!doc.block("header").unwrap().is_dirty());
        assert_eq!(doc.dirty_neg_blocks().len(), 1);
    }

    #[test]
    fn marker_hash_matching_actual_hash_is_not_dirty() {
        let body = "인공물보다 방치된 것을 고른다.\n습도로 공간을 읽는다.";
        let h = block_hash(body);
        let text =
            format!("<!-- soul:neg id=profile rev=3 hash={h} -->\n{body}\n<!-- /soul:neg -->\n");
        let doc = parse(&text).unwrap();
        assert!(!doc.block("profile").unwrap().is_dirty());
        assert!(doc.dirty_neg_blocks().is_empty());
    }

    // ---------- §8.3 규칙 3 — 해시 정규화 ----------

    #[test]
    fn normalize_drops_only_outer_blank_lines() {
        // 앞뒤의 공백만 있는 줄은 사라지고, 각 줄 뒤쪽 공백도 사라진다.
        assert_eq!(
            normalize_for_hash("\n  \n첫 줄   \n둘째 줄\t\n \n\n"),
            "첫 줄\n둘째 줄"
        );
        // 전부 공백이면 빈 문자열.
        assert_eq!(normalize_for_hash("\n   \n\t\n"), "");
        assert_eq!(normalize_for_hash(""), "");
    }

    #[test]
    fn normalize_preserves_inner_blank_lines() {
        assert_eq!(normalize_for_hash("a\n\nb"), "a\n\nb");
        // 내부 빈 줄이 공백으로 채워져 있어도 빈 줄로 남는다 (제거되지 않는다).
        assert_eq!(normalize_for_hash("a\n   \nb"), "a\n\nb");
    }

    #[test]
    fn hash_is_stable_against_outer_blank_lines_and_trailing_spaces() {
        let base = block_hash("첫 줄\n둘째 줄");
        assert_eq!(block_hash("\n\n첫 줄\n둘째 줄\n\n  \n"), base);
        assert_eq!(block_hash("첫 줄   \n둘째 줄\t"), base);
        // 렌더러가 붙이는 끝 개행 하나도 해시를 바꾸지 않는다.
        assert_eq!(block_hash("첫 줄\n둘째 줄\n"), base);
    }

    #[test]
    fn hash_changes_when_inner_blank_line_changes() {
        assert_ne!(block_hash("a\nb"), block_hash("a\n\nb"));
        assert_ne!(block_hash("a\n\nb"), block_hash("a\n\n\nb"));
    }

    #[test]
    fn crlf_hashes_the_same_as_lf() {
        assert_eq!(normalize_for_hash("a\r\nb\r\n"), "a\nb");
        assert_eq!(block_hash("a\r\nb\r\n"), block_hash("a\nb"));
    }

    #[test]
    fn hash_is_six_lowercase_hex_chars() {
        let h = block_hash("아무 본문");
        assert_eq!(h.len(), 6);
        assert!(h
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        // 빈 본문의 해시는 sha256("") 앞 6자다.
        assert_eq!(block_hash("   \n\n"), "e3b0c4");
    }

    #[test]
    fn actual_hash_and_normalized_body_use_rule_three() {
        let text =
            "<!-- soul:neg id=p rev=1 hash=000000 -->\n\n  본문 줄  \n\n<!-- /soul:neg -->\n";
        let doc = parse(text).unwrap();
        let b = doc.block("p").unwrap();
        assert_eq!(b.body, "\n  본문 줄  \n\n");
        assert_eq!(b.normalized_body(), "  본문 줄");
        assert_eq!(b.actual_hash(), block_hash("  본문 줄"));
    }

    // ---------- §8.3 규칙 2 — 마커 줄 판별 ----------

    #[test]
    fn marker_line_allows_surrounding_whitespace_only() {
        let text = "   <!-- soul:gen id=header -->  \n본문\n\t<!-- /soul:gen -->\t\n";
        let doc = parse(text).unwrap();
        assert_eq!(doc.blocks.len(), 1);
        assert_eq!(doc.blocks[0].body, "본문\n");
    }

    #[test]
    fn marker_with_other_characters_on_the_line_is_prose() {
        // 앞에 글자가 붙으면 마커가 아니라 본문이다 → 블록이 하나도 없다.
        let text = "x <!-- soul:gen id=header -->\n본문\n";
        let doc = parse(text).unwrap();
        assert!(doc.blocks.is_empty());
        // 뒤에 붙어도 마찬가지다.
        let text = "<!-- soul:gen id=header --> x\n";
        assert!(parse(text).unwrap().blocks.is_empty());
    }

    #[test]
    fn ordinary_html_comments_are_prose() {
        let text = "<!-- 그냥 주석 -->\n<!-- soul:gen id=a -->\n본문\n<!-- /soul:gen -->\n";
        let doc = parse(text).unwrap();
        assert_eq!(doc.blocks.len(), 1);
        assert_eq!(doc.blocks[0].id.as_deref(), Some("a"));
    }

    #[test]
    fn malformed_soul_marker_is_a_parse_error() {
        // id= 없음
        assert_eq!(parse_err_line("<!-- soul:gen -->\n<!-- /soul:gen -->\n"), 1);
        // neg에 hash= 없음
        assert_eq!(
            parse_err_line("<!-- soul:neg id=p rev=1 -->\n<!-- /soul:neg -->\n"),
            1
        );
        // rev가 정수가 아님
        assert_eq!(
            parse_err_line("<!-- soul:neg id=p rev=x hash=abc123 -->\n<!-- /soul:neg -->\n"),
            1
        );
        // human에 속성이 붙음
        assert_eq!(
            parse_err_line("<!-- soul:human id=p -->\n<!-- /soul:human -->\n"),
            1
        );
        // 닫는 마커에 속성이 붙음
        assert_eq!(
            parse_err_line("<!-- soul:gen id=a -->\n<!-- /soul:gen id=a -->\n"),
            2
        );
    }

    // ---------- §8.3 규칙 5 — soul:human 은 통째로 건너뛴다 ----------

    #[test]
    fn human_body_is_not_parsed_at_all() {
        let text = concat!(
            "<!-- soul:human -->\n",
            "<!-- soul:gen id=fake -->\n",
            "마커처럼 생긴 줄도 그냥 내 문장이다\n",
            "<!-- /soul:neg -->\n",
            "<!-- /soul:human -->\n"
        );
        let doc = parse(text).expect("human 본문 안의 마커는 파싱하지 않는다");
        assert_eq!(doc.blocks.len(), 1);
        assert_eq!(doc.blocks[0].kind, BlockKind::Human);
        assert!(doc.block("fake").is_none());
        assert_eq!(
            doc.human_body(),
            Some("<!-- soul:gen id=fake -->\n마커처럼 생긴 줄도 그냥 내 문장이다\n<!-- /soul:neg -->\n")
        );
    }

    #[test]
    fn unclosed_human_block_is_still_an_error() {
        // 건너뛴다고 해서 닫히지 않은 것을 봐주지는 않는다.
        assert_eq!(parse_err_line("서문\n<!-- soul:human -->\n내 문장\n"), 2);
    }

    // ---------- §8.3 규칙 6 · T10 — 마커 짝 불일치 ----------

    #[test]
    fn t10_unclosed_marker_fails_and_reports_opening_line() {
        let text = "# SOUL\n\n<!-- soul:gen id=header -->\n본문\n\n## 다음 절\n";
        assert_eq!(parse_err_line(text), 3);
    }

    #[test]
    fn t10_unmatched_closing_marker_fails() {
        let text = "# SOUL\n<!-- /soul:gen -->\n";
        assert_eq!(parse_err_line(text), 2);
    }

    #[test]
    fn t10_nested_marker_fails() {
        let text = "<!-- soul:gen id=a -->\n<!-- soul:neg id=b rev=1 hash=abc123 -->\n본문\n<!-- /soul:neg -->\n<!-- /soul:gen -->\n";
        assert_eq!(parse_err_line(text), 2);
        // 같은 종류의 중첩도 잡는다.
        let text = "<!-- soul:gen id=a -->\n<!-- soul:gen id=b -->\n<!-- /soul:gen -->\n";
        assert_eq!(parse_err_line(text), 2);
    }

    #[test]
    fn t10_crossed_close_kind_fails() {
        let text = "<!-- soul:gen id=a -->\n본문\n<!-- /soul:neg -->\n";
        assert_eq!(parse_err_line(text), 3);
    }

    #[test]
    fn t10_does_not_auto_repair() {
        // 파싱이 실패하면 부분 결과를 돌려주지 않는다 — 호출자가 파일을 쓰지 않도록.
        let text = "<!-- soul:gen id=ok -->\n본문\n<!-- /soul:gen -->\n<!-- soul:neg id=b rev=1 hash=abc123 -->\n";
        assert!(parse(text).is_err());
    }

    // ---------- 기타 ----------

    #[test]
    fn document_without_trailing_newline_parses() {
        let text = "<!-- soul:gen id=a -->\n본문\n<!-- /soul:gen -->";
        let doc = parse(text).unwrap();
        assert_eq!(doc.blocks.len(), 1);
        assert_eq!(doc.blocks[0].body, "본문\n");
        assert_eq!(doc.raw, text);
    }

    #[test]
    fn empty_block_has_empty_body() {
        let text = "<!-- soul:human -->\n<!-- /soul:human -->\n";
        let doc = parse(text).unwrap();
        assert_eq!(doc.human_body(), Some(""));
        let text = "<!-- soul:gen id=a -->\n<!-- /soul:gen -->\n";
        let doc = parse(text).unwrap();
        assert_eq!(doc.blocks[0].body, "");
        assert_eq!(doc.blocks[0].actual_hash(), "e3b0c4");
    }

    #[test]
    fn empty_document_yields_no_blocks() {
        let doc = parse("").unwrap();
        assert!(doc.blocks.is_empty());
        assert_eq!(doc.raw, "");
    }

    #[test]
    fn crlf_document_keeps_raw_but_hashes_as_lf() {
        let text = "<!-- soul:neg id=p rev=1 hash=000000 -->\r\n첫 줄\r\n둘째 줄\r\n<!-- /soul:neg -->\r\n";
        let doc = parse(text).unwrap();
        let b = doc.block("p").unwrap();
        // 본문은 원문 그대로 (CRLF 포함).
        assert_eq!(b.body, "첫 줄\r\n둘째 줄\r\n");
        assert_eq!(b.actual_hash(), block_hash("첫 줄\n둘째 줄"));
        assert_eq!(doc.raw, text);
    }
}
