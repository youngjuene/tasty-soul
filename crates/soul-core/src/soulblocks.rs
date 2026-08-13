//! 목적지별 블록 조립 (§D4). **이 모듈이 §18-4를 막는 유일한 장치다.**
//!
//! > 규칙의 기준은 블록이 아니라 **목적지**다.
//!
//! | 목적지 | `soul:human` |
//! |---|---|
//! | OpenAI 성찰 호출 (`read_soul`, §10.4 프롬프트) | **제외한다** |
//! | 로컬 MCP 서버 `soul://md` (§19) | **포함한다** |
//!
//! 원격 모델에게 보여주는 순간 "시스템 언어에 오염되지 않은 자기 문장"이라는 목적이 사라진다.
//! 반대로 로컬 에이전트는 기기를 떠나지 않으므로 포함하는 편이 낫다.
//!
//! **`SOUL.md`를 파일째로 읽어 프롬프트에 넣는 구현을 하지 말 것.**
//! 반드시 블록 단위로 조립하고, 조립하는 쪽에서 목적지를 인자로 받는다 (T29·T30·T31).

use crate::error::Result;
use crate::soulmd::{BlockKind, SoulDoc};
use std::collections::BTreeMap;

/// 조립 결과가 어디로 가는가.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Destination {
    /// 원격 API 호출. `soul:human`을 **제외한다.**
    Remote,
    /// 로컬 MCP 서버·로컬 내보내기. `soul:human`을 **포함한다.**
    Local,
}

/// §11.2 `read_soul` 툴의 반환 형태: `{block_id: {text, hash}}`.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BlockView {
    pub text: String,
    pub hash: String,
}

/// `soul:human` 마커에는 `id=`가 없으므로(§8.1) 조립할 때 키 이름을 하나 정해야 한다.
///
/// `Destination::Remote`에서는 이 키가 **만들어지지 않는다** — 빈 값으로도 넣지 않는다 (§D4, T29).
pub const HUMAN_KEY: &str = "human";

/// §19.8 출력 맨 앞에 붙는 고지. 마크다운 주석이므로 어느 렌더러에서도 보이지 않지만
/// 프롬프트 원문에는 남아, 이 경로가 어떤 경로인지 모델과 사람 모두가 읽을 수 있다.
const EXPORT_HEADER: &str = concat!(
    "<!-- soul export --target=prompt (§19.8) — `SOUL.md` 원문에서 마커 주석 줄만 제거한 것입니다. ",
    "번역·요약·축약을 하지 않았습니다. -->\n",
    "<!-- 이것은 축소된 경로이며 §19.1의 한계를 그대로 갖습니다: 프롬프트는 압축이라 관측 대부분이 버려지고, ",
    "로그는 자라지만 프롬프트는 자라지 못하며, 모델이 근거 관측을 되짚을 수단이 없습니다. ",
    "정식 경로는 `soul mcp`(§19)입니다. -->\n",
    "\n",
);

/// 블록을 목적지 규칙에 따라 조립한다.
///
/// `Destination::Remote`이면 `soul:human` 블록은 결과에 **존재하지 않는다** —
/// 빈 문자열로 넣지도 않는다. 키 자체가 없어야 실수로 붙는 일이 없다.
pub fn assemble(doc: &SoulDoc, dest: Destination) -> BTreeMap<String, BlockView> {
    let mut out = BTreeMap::new();
    for b in &doc.blocks {
        let key = match b.kind {
            BlockKind::Human => match dest {
                // 여기서 `continue` 하는 것이 §D4의 전부다. 값을 비우는 것이 아니라
                // 맵에 키를 만들지 않는다 (T29).
                Destination::Remote => continue,
                Destination::Local => HUMAN_KEY.to_string(),
            },
            BlockKind::Gen | BlockKind::Neg => match b.id.as_deref() {
                Some(id) => id.to_string(),
                // id 없는 gen/neg는 파서가 이미 거부한다 (§8.3 규칙 6). 방어적으로 건너뛴다.
                None => continue,
            },
        };
        out.insert(
            key,
            BlockView {
                text: b.normalized_body(),
                // §11.2 `propose_delta`의 `from_hash` 대조 대상이다. 마커가 주장하는 값이
                // 아니라 **본문의 실제 해시**여야 한다.
                hash: b.actual_hash(),
            },
        );
    }
    out
}

/// §19.8 `soul export --target=prompt`.
///
/// `SOUL.md` 원문에서 **마커 주석만 제거**한 것을 출력한다.
/// **번역·요약·축약을 하지 않는다.** 축소된 경로라는 사실을 출력 헤더에 주석으로 적는다.
pub fn export_prompt(doc: &SoulDoc) -> Result<String> {
    let mut out = String::with_capacity(doc.raw.len() + EXPORT_HEADER.len());
    out.push_str(EXPORT_HEADER);

    // 원문을 줄 단위로 훑어 마커 줄만 버린다. 나머지 줄은 **한 글자도 건드리지 않는다.**
    // `split('\n')`은 마지막 개행 뒤의 빈 조각도 내주므로, join으로 되돌리면
    // 원문의 끝 개행 유무가 그대로 보존된다.
    let kept: Vec<&str> = doc
        .raw
        .split('\n')
        .filter(|line| !is_marker_line(line))
        .collect();
    out.push_str(&kept.join("\n"));
    Ok(out)
}

/// `<!-- soul:gen ... -->` / `<!-- soul:neg ... -->` / `<!-- soul:human -->` 와 각 닫는 줄.
///
/// §8.3 규칙 2와 같이 앞뒤 공백만 허용한다. 마커가 아닌 일반 주석은 남긴다 —
/// 지우는 것은 마커뿐이다.
fn is_marker_line(line: &str) -> bool {
    let t = line.trim();
    if t.len() < 7 || !t.starts_with("<!--") || !t.ends_with("-->") {
        return false;
    }
    let inner = t[4..t.len() - 3].trim();
    let inner = inner.strip_prefix('/').unwrap_or(inner);
    let name = inner.split_whitespace().next().unwrap_or("");
    matches!(name, "soul:gen" | "soul:neg" | "soul:human")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soulmd::SoulBlock;

    const HUMAN_TEXT: &str = "아무한테도 안 보여줄 문장. 습도라는 말이 자꾸 걸린다.";
    const PROFILE_TEXT: &str = "인공물보다 방치된 것을 고른다.";

    fn raw_doc() -> String {
        format!(
            "# SOUL\n\
             \n\
             <!-- soul:gen id=header -->\n\
             기준 2026-08-13 · 관측 342 · 최초 2026-03-02\n\
             <!-- /soul:gen -->\n\
             \n\
             ## 지금의 취향\n\
             \n\
             <!-- soul:neg id=profile rev=17 hash=a3f91c -->\n\
             {PROFILE_TEXT}\n\
             <!-- /soul:neg -->\n\
             \n\
             ## 내가 쓴 것\n\
             \n\
             <!-- soul:human -->\n\
             {HUMAN_TEXT}\n\
             <!-- /soul:human -->\n"
        )
    }

    /// 파서를 거치지 않고 직접 만든다 — 이 모듈의 규칙만 시험한다.
    fn doc() -> SoulDoc {
        SoulDoc {
            blocks: vec![
                SoulBlock {
                    kind: BlockKind::Gen,
                    id: Some("header".into()),
                    rev: None,
                    hash: None,
                    body: "기준 2026-08-13 · 관측 342 · 최초 2026-03-02\n".into(),
                    line: 3,
                },
                SoulBlock {
                    kind: BlockKind::Neg,
                    id: Some("profile".into()),
                    rev: Some(17),
                    hash: Some("a3f91c".into()),
                    body: format!("{PROFILE_TEXT}\n"),
                    line: 9,
                },
                SoulBlock {
                    kind: BlockKind::Human,
                    id: None,
                    rev: None,
                    hash: None,
                    body: format!("{HUMAN_TEXT}\n"),
                    line: 15,
                },
            ],
            raw: raw_doc(),
        }
    }

    #[test]
    fn t29_remote_assembly_has_no_human_key_at_all() {
        let out = assemble(&doc(), Destination::Remote);
        assert!(
            !out.contains_key(HUMAN_KEY),
            "원격 조립에 human 키가 있으면 안 된다: {:?}",
            out.keys().collect::<Vec<_>>()
        );
        // 빈 문자열로 들어가 있지도 않아야 한다 — 키 자체가 없어야 한다.
        assert_eq!(out.len(), 2, "gen header + neg profile 둘뿐이다");
        assert!(out.contains_key("header") && out.contains_key("profile"));
        // 어떤 블록에도 사람이 쓴 문장이 섞이지 않아야 한다 (§18-4).
        for (k, v) in &out {
            assert!(!v.text.contains(HUMAN_TEXT), "{k} 블록에 soul:human이 샜다");
        }
    }

    #[test]
    fn t30_local_assembly_includes_human() {
        let out = assemble(&doc(), Destination::Local);
        let human = out
            .get(HUMAN_KEY)
            .expect("로컬 조립에는 human이 있어야 한다");
        assert!(human.text.contains(HUMAN_TEXT));
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn hash_is_the_actual_body_hash_not_the_marker_claim() {
        // 마커는 a3f91c라고 주장하지만 본문의 실제 해시를 실어야 §11.2 from_hash 대조가 성립한다.
        let out = assemble(&doc(), Destination::Remote);
        let p = &out["profile"];
        assert_eq!(
            p.hash,
            crate::soulmd::block_hash(&format!("{PROFILE_TEXT}\n"))
        );
        assert_ne!(p.hash, "a3f91c");
    }

    #[test]
    fn export_removes_only_marker_lines() {
        let d = doc();
        let s = export_prompt(&d).unwrap();

        // 헤더는 축소된 경로임을 밝히는 마크다운 주석이다.
        assert!(s.starts_with("<!--"), "출력 맨 앞이 주석이어야 한다: {s}");
        assert!(s.contains("축소된 경로"));
        assert!(s.contains("§19.1"));

        // 헤더를 걷어낸 본문은 원문에서 마커 줄만 빠진 것과 정확히 같아야 한다.
        let body = s.strip_prefix(EXPORT_HEADER).expect("헤더 뒤가 본문이다");
        let expected: Vec<&str> = d.raw.split('\n').filter(|l| !is_marker_line(l)).collect();
        assert_eq!(body, expected.join("\n"));

        // 마커는 하나도 남지 않는다.
        for m in ["soul:gen", "soul:neg", "soul:human", "/soul:"] {
            assert!(!body.contains(m), "본문에 마커가 남았다: {m}\n{body}");
        }
        // 번역·요약·축약 없음 — 원문 문장이 글자 그대로 남는다.
        assert!(body.contains(PROFILE_TEXT));
        assert!(body.contains(HUMAN_TEXT));
        assert!(body.contains("# SOUL"));
        assert!(body.contains("## 지금의 취향"));
        assert!(body.contains("## 내가 쓴 것"));
    }

    #[test]
    fn export_keeps_non_marker_comments() {
        let mut d = doc();
        d.raw = "# SOUL\n\n<!-- 사람이 남긴 메모 -->\n<!-- soul:human -->\n본문\n<!-- /soul:human -->\n".into();
        let s = export_prompt(&d).unwrap();
        assert!(
            s.contains("<!-- 사람이 남긴 메모 -->"),
            "마커가 아닌 주석은 남는다"
        );
        assert!(!s.contains("soul:human"));
    }

    #[test]
    fn marker_line_detection_follows_parser_rule_2() {
        assert!(is_marker_line("<!-- soul:gen id=header -->"));
        assert!(is_marker_line("   <!-- soul:gen id=header -->   "));
        assert!(is_marker_line("<!-- /soul:gen -->"));
        assert!(is_marker_line(
            "<!-- soul:neg id=profile rev=17 hash=a3f91c -->"
        ));
        assert!(is_marker_line("<!-- soul:human -->"));
        assert!(is_marker_line("<!-- /soul:human -->"));
        // 마커 줄 앞에 다른 글자가 있으면 마커가 아니다 (§8.3 규칙 2).
        assert!(!is_marker_line("x <!-- soul:gen id=header -->"));
        assert!(!is_marker_line("<!-- 그냥 주석 -->"));
        assert!(!is_marker_line("일반 문장"));
        assert!(!is_marker_line(""));
    }
}
