//! `SOUL.md` 렌더러 (§8.2 · §8.2.1).
//!
//! 출력은 §8.2 템플릿과 **구조가 정확히 같아야 한다.** T1(바이트 동일성)이 여기에 걸린다.
//!
//! ```markdown
//! # SOUL
//!
//! <!-- soul:gen id=header -->
//! 기준 2026-08-13 · 관측 342 · 최초 2026-03-02
//! 어긋남 0.31 · 해상도 0.58
//! <!-- /soul:gen -->
//!
//! ## 지금의 취향
//!
//! <!-- soul:neg id=profile rev=17 hash=a3f91c -->
//! ...
//! <!-- /soul:neg -->
//!
//! ## 축
//!
//! <!-- soul:gen id=axes -->
//! | 축 | 값 | 90일 변화 |
//! |---|---|---|
//! | chroma | 0.34 | — |
//! ...
//! <!-- /soul:gen -->
//!
//! ## 어긋남
//!
//! <!-- soul:gen id=divergence -->
//! - 일관성 0.62 · 정정 18건
//! - 대표 사례: 01J8XQB / 01J8XQC / 01J8XQD
//! <!-- /soul:gen -->
//!
//! ## 내가 쓴 것
//!
//! <!-- soul:human -->
//!
//! <!-- /soul:human -->
//! ```
//!
//! ## 렌더링 규칙 (§8.2.1)
//!
//! - `axes` 표는 **8축 전부**를 §7 순서 그대로. 부분 집합 금지.
//! - 축 값은 소수 둘째 자리, 변화량은 부호를 붙여 소수 둘째 자리 (`soulmd::fmt_*` 사용).
//! - `null`은 `—` (§R10).
//! - 헤더의 `어긋남`은 `read`가 아닌 셀의 비율, `해상도`는 `T_ref`가 속한 달의 `crystal`.
//! - 날짜는 `YYYY-MM-DD`, UTC 기준.
//! - `divergence` 블록의 대표 사례가 없으면 **그 줄 자체를 생략한다.**
//! - 파일은 LF로 끝난다. BOM 없음.

use crate::derived::Derived;
use crate::ids::ObsId;
use crate::obs::Axis;
use crate::soulmd::parse::block_hash;
use crate::soulmd::{fmt_change, fmt_value, NULL_GLYPH, SEP};

/// 렌더 입력. 파생값과 사람이 쓴 블록을 **분리해서** 받는다.
///
/// `soul:human`을 여기서 명시적으로 받는 이유: 재렌더가 기존 파일을 읽지 않고 새로 쓰면
/// 사용자의 자유 기록이 조용히 사라진다 (§18-5, §R2, T4/T4b).
pub struct RenderInput<'a> {
    pub derived: &'a Derived,
    /// `soul:neg id=profile` 본문. 관측 재생 결과이거나 편집 결과다.
    pub profile_text: &'a str,
    /// `profile` 블록의 `rev`. `profile_edit`/`soul_delta` 누적 횟수.
    pub profile_rev: u32,
    /// `soul:human` 본문 **원문 그대로**. 재빌드 시 기존 파일에서 이월한 값이다 (§R2).
    pub human_text: &'a str,
    /// 대표 사례 ID (§12.6). 비어 있으면 그 줄을 생략한다.
    pub divergence_examples: &'a [ObsId],
}

/// `SOUL.md` 전문을 만든다. `profile` 블록의 `hash`는 여기서 계산해 마커에 박는다.
pub fn render(input: &RenderInput<'_>) -> String {
    let d = input.derived;
    let mut out = String::new();

    out.push_str("# SOUL\n\n");

    // ── header (soul:gen)
    out.push_str("<!-- soul:gen id=header -->\n");
    out.push_str(&header_body(d));
    out.push_str("<!-- /soul:gen -->\n\n");

    // ── profile (soul:neg) — 마커의 hash는 원문 기준으로 계산한다 (§8.3 규칙 3·4).
    out.push_str("## 지금의 취향\n\n");
    out.push_str("<!-- soul:neg id=profile rev=");
    out.push_str(&input.profile_rev.to_string());
    out.push_str(" hash=");
    out.push_str(&block_hash(input.profile_text));
    out.push_str(" -->\n");
    out.push_str(&block_body(input.profile_text));
    out.push_str("<!-- /soul:neg -->\n\n");

    // ── axes (soul:gen)
    out.push_str("## 축\n\n");
    out.push_str("<!-- soul:gen id=axes -->\n");
    out.push_str(&axes_body(d));
    out.push_str("<!-- /soul:gen -->\n\n");

    // ── divergence (soul:gen)
    out.push_str("## 어긋남\n\n");
    out.push_str("<!-- soul:gen id=divergence -->\n");
    out.push_str(&divergence_body(d, input.divergence_examples));
    out.push_str("<!-- /soul:gen -->\n\n");

    // ── human — 원문 그대로 이월한다. 렌더러는 내용을 해석하지 않는다 (§R2, T4).
    out.push_str("## 내가 쓴 것\n\n");
    out.push_str("<!-- soul:human -->\n");
    out.push_str(&block_body(input.human_text));
    out.push_str("<!-- /soul:human -->\n");

    out
}

/// 전 블록이 비어 있는 최초 스켈레톤 (§3 최초 실행 3단계).
pub fn skeleton() -> String {
    // 파생값이 하나도 없는 상태 = `Derived::default()`. 전부 `null`이므로 §R10대로 `—`가 된다.
    let derived = Derived::default();
    render(&RenderInput {
        derived: &derived,
        profile_text: "",
        profile_rev: 0,
        human_text: "",
        divergence_examples: &[],
    })
}

// ────────────────────────────────────────────────────────────── 블록 본문

/// `기준 … · 관측 N · 최초 …` / `어긋남 x · 해상도 y` 두 줄 (§8.2).
///
/// `관측`은 **활성 ingest 수**다 (`observation_count`, §R9 / OPEN-DECISIONS #18).
/// 관측이 하나도 없으면 `T_ref`·`T_first`가 `None`이므로 날짜 자리도 `—`가 된다.
fn header_body(d: &Derived) -> String {
    let t_ref = date_or_null(d.t_ref);
    let t_first = date_or_null(d.t_first);
    let n = d.observation_count;
    let misread = fmt_value(d.misread_ratio);
    let crystal = fmt_value(d.crystal_now);
    format!("기준 {t_ref}{SEP}관측 {n}{SEP}최초 {t_first}\n어긋남 {misread}{SEP}해상도 {crystal}\n")
}

/// 8축 표 (§8.2.1). **`Axis::ALL` 순서 그대로 8행 전부**를 싣는다. 부분 집합 금지.
fn axes_body(d: &Derived) -> String {
    let mut s = String::from("| 축 | 값 | 90일 변화 |\n|---|---|---|\n");
    for a in Axis::ALL {
        let value = fmt_value(d.axes_final.map(|ax| ax.get(a)));
        let change = fmt_change(d.axes_change[a.index()]);
        s.push_str(&format!("| {} | {} | {} |\n", a.name(), value, change));
    }
    s
}

/// `- 일관성 … · 정정 …건` (+ 대표 사례 줄).
///
/// `일관성`은 `coherence_sensory.value`, `정정`은 **두 층 합쳐** `prose ≠ null`인
/// reading 수(`Derived::corrections_total`)다 (OPEN-DECISIONS #17).
/// `일관성`은 비율이라 표본이 모자라면 `—`지만 `정정`은 개수이므로 `0건`으로 렌더한다.
/// 대표 사례가 없으면 **그 줄 자체를 생략한다** (§8.2.1).
fn divergence_body(d: &Derived, examples: &[ObsId]) -> String {
    let coherence = fmt_value(d.coherence_sensory.as_ref().map(|c| c.value));
    let corrections = d.corrections_total;
    let mut s = format!("- 일관성 {coherence}{SEP}정정 {corrections}건\n");
    if !examples.is_empty() {
        let ids: Vec<&str> = examples.iter().map(|i| i.as_str()).collect();
        s.push_str(&format!("- 대표 사례: {}\n", ids.join(" / ")));
    }
    s
}

/// `null`인 타임스탬프는 `—` (§R10).
fn date_or_null(ts: Option<crate::time::Ts>) -> String {
    match ts {
        Some(t) => t.date_string(),
        None => NULL_GLYPH.to_string(),
    }
}

/// 마커 사이에 실을 본문을 다듬는다.
///
/// (a) CRLF→LF (b) 앞뒤의 공백만 있는 줄 제거 (c) 각 줄 뒤쪽 공백 제거 —
/// §8.3 규칙 3의 해시 정규화와 **같은 규칙**이라 다시 파싱해도 해시가 변하지 않는다.
/// 본문이 비면 빈 줄 하나를 남긴다 (§8.2 템플릿의 `soul:human` 모양).
fn block_body(text: &str) -> String {
    let lf = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<&str> = lf.lines().collect();
    while lines.first().is_some_and(|l| l.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    if lines.is_empty() {
        return "\n".to_string();
    }
    let mut s = String::new();
    for l in lines {
        s.push_str(l.trim_end());
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derived::Coherence;
    use crate::obs::Axes;
    use crate::time::Ts;

    const EX1: &str = "01J8XQZK3M7P4RSTVWXYZ01234";
    const EX2: &str = "01J8XQZK3M7P4RSTVWXYZ01235";
    const EX3: &str = "01J8XQZK3M7P4RSTVWXYZ01236";

    const PROFILE: &str = "인공물보다 방치된 것을 고른다. 채도가 아니라 습도로\n공간을 읽고, 사람이 방금 나간 자리에 반복해서 멈춘다.";

    fn ids() -> Vec<ObsId> {
        [EX1, EX2, EX3]
            .iter()
            .map(|s| ObsId::parse(s).expect("테스트 ULID는 유효하다"))
            .collect()
    }

    /// §8.2 템플릿에 적힌 값 그대로.
    fn spec_derived() -> Derived {
        Derived {
            t_ref: Ts::parse("2026-08-13T09:12:33.123Z"),
            t_first: Ts::parse("2026-03-02T01:00:00.000Z"),
            observation_count: 342,
            total_observation_count: 901,
            axes_final: Some(Axes {
                chroma: 0.34,
                luminance: 0.52,
                density: 0.28,
                grain: 0.71,
                tempo: 0.19,
                space: 0.66,
                valence: 0.41,
                intensity: 0.30,
            }),
            axes_change: [
                None,
                Some(0.03),
                Some(-0.06),
                Some(0.14),
                None,
                Some(0.02),
                Some(-0.11),
                Some(0.05),
            ],
            misread_ratio: Some(0.31),
            crystal_now: Some(0.58),
            // 정정 18건 — `prose != null` 인 reading 수(두 층 합). OPEN-DECISIONS #17.
            // `Coherence.sample` 이 아니라 `corrections_total` 에서 온다:
            // 표본이 2건 미만이면 `Coherence` 가 통째로 `None` 이라 개수를 잃는다.
            corrections_total: 18,
            coherence_sensory: Some(Coherence {
                value: 0.62,
                sample: 12,
                examples: ids(),
                systematic: true,
            }),
            coherence_cultural: Some(Coherence {
                value: 0.44,
                sample: 6,
                examples: vec![],
                systematic: true,
            }),
            ..Default::default()
        }
    }

    /// 블록 구조만 남긴다 — 본문 값은 빼고 제목·마커 줄만 순서대로.
    fn structure(md: &str) -> Vec<&str> {
        md.lines()
            .filter(|l| l.starts_with("<!--") || l.starts_with('#'))
            .collect()
    }

    const SPEC_STRUCTURE: [&str; 14] = [
        "# SOUL",
        "<!-- soul:gen id=header -->",
        "<!-- /soul:gen -->",
        "## 지금의 취향",
        "<!-- /soul:neg -->",
        "## 축",
        "<!-- soul:gen id=axes -->",
        "<!-- /soul:gen -->",
        "## 어긋남",
        "<!-- soul:gen id=divergence -->",
        "<!-- /soul:gen -->",
        "## 내가 쓴 것",
        "<!-- soul:human -->",
        "<!-- /soul:human -->",
    ];

    /// `soul:neg` 여는 마커는 rev·hash가 붙으므로 구조 비교에서 빼고 따로 본다.
    fn structure_without_neg_open(md: &str) -> Vec<&str> {
        structure(md)
            .into_iter()
            .filter(|l| !l.starts_with("<!-- soul:neg"))
            .collect()
    }

    #[test]
    fn renders_spec_template_byte_for_byte() {
        // T1 — §8.2 템플릿과 줄 구조·값 형식이 정확히 같아야 한다.
        let d = spec_derived();
        let out = render(&RenderInput {
            derived: &d,
            profile_text: PROFILE,
            profile_rev: 17,
            human_text: "",
            divergence_examples: &ids(),
        });
        let expected = format!(
            "# SOUL\n\
             \n\
             <!-- soul:gen id=header -->\n\
             기준 2026-08-13 · 관측 342 · 최초 2026-03-02\n\
             어긋남 0.31 · 해상도 0.58\n\
             <!-- /soul:gen -->\n\
             \n\
             ## 지금의 취향\n\
             \n\
             <!-- soul:neg id=profile rev=17 hash={hash} -->\n\
             {PROFILE}\n\
             <!-- /soul:neg -->\n\
             \n\
             ## 축\n\
             \n\
             <!-- soul:gen id=axes -->\n\
             | 축 | 값 | 90일 변화 |\n\
             |---|---|---|\n\
             | chroma | 0.34 | — |\n\
             | luminance | 0.52 | +0.03 |\n\
             | density | 0.28 | \u{2212}0.06 |\n\
             | grain | 0.71 | +0.14 |\n\
             | tempo | 0.19 | — |\n\
             | space | 0.66 | +0.02 |\n\
             | valence | 0.41 | \u{2212}0.11 |\n\
             | intensity | 0.30 | +0.05 |\n\
             <!-- /soul:gen -->\n\
             \n\
             ## 어긋남\n\
             \n\
             <!-- soul:gen id=divergence -->\n\
             - 일관성 0.62 · 정정 18건\n\
             - 대표 사례: {EX1} / {EX2} / {EX3}\n\
             <!-- /soul:gen -->\n\
             \n\
             ## 내가 쓴 것\n\
             \n\
             <!-- soul:human -->\n\
             \n\
             <!-- /soul:human -->\n",
            hash = block_hash(PROFILE),
        );
        assert_eq!(out, expected);
    }

    #[test]
    fn all_eight_axes_present_in_spec_order() {
        let d = spec_derived();
        let out = render(&RenderInput {
            derived: &d,
            profile_text: PROFILE,
            profile_rev: 1,
            human_text: "",
            divergence_examples: &[],
        });
        let rows: Vec<&str> = out
            .lines()
            .filter(|l| l.starts_with("| ") && !l.starts_with("| 축 "))
            .collect();
        assert_eq!(rows.len(), 8, "8축 전부여야 한다: {rows:?}");
        for (row, axis) in rows.iter().zip(Axis::ALL) {
            assert!(
                row.starts_with(&format!("| {} |", axis.name())),
                "§7 순서가 어긋났다: {row}"
            );
        }
    }

    #[test]
    fn nulls_render_as_em_dash() {
        // §R10 — 0으로 대체하거나 항목을 생략하지 않는다.
        let d = Derived::default();
        let out = render(&RenderInput {
            derived: &d,
            profile_text: "",
            profile_rev: 0,
            human_text: "",
            divergence_examples: &[],
        });
        assert!(out.contains("기준 — · 관측 0 · 최초 —\n"), "{out}");
        assert!(out.contains("어긋남 — · 해상도 —\n"), "{out}");
        assert!(out.contains("- 일관성 — · 정정 0건\n"), "{out}");
        for a in Axis::ALL {
            assert!(
                out.contains(&format!("| {} | — | — |\n", a.name())),
                "{} 행이 —가 아니다\n{out}",
                a.name()
            );
        }
    }

    #[test]
    fn example_line_is_dropped_when_there_are_no_examples() {
        // §8.2.1 — 대표 사례가 없으면 그 줄 자체를 생략한다.
        let d = spec_derived();
        let with = render(&RenderInput {
            derived: &d,
            profile_text: PROFILE,
            profile_rev: 3,
            human_text: "",
            divergence_examples: &ids(),
        });
        let without = render(&RenderInput {
            derived: &d,
            profile_text: PROFILE,
            profile_rev: 3,
            human_text: "",
            divergence_examples: &[],
        });
        assert!(with.contains("- 대표 사례: "));
        assert!(!without.contains("대표 사례"), "{without}");
        // 줄 하나만 사라지고 나머지는 그대로다.
        assert_eq!(without.lines().count() + 1, with.lines().count());
        assert!(without.contains("- 일관성 0.62 · 정정 18건\n<!-- /soul:gen -->"));
    }

    #[test]
    fn human_block_is_carried_verbatim() {
        // T4 — 사용자의 자유 기록은 렌더러가 해석하지 않고 그대로 이월한다 (§R2).
        let d = spec_derived();
        let human = "2026-03-02\n비 온 다음 날의 공기.\n\n— 왜 자꾸 같은 골목을 찍는가";
        let out = render(&RenderInput {
            derived: &d,
            profile_text: PROFILE,
            profile_rev: 2,
            human_text: human,
            divergence_examples: &[],
        });
        assert!(
            out.contains(&format!(
                "<!-- soul:human -->\n{human}\n<!-- /soul:human -->\n"
            )),
            "{out}"
        );
    }

    #[test]
    fn crlf_input_is_normalized_and_file_ends_with_single_lf() {
        let d = spec_derived();
        let out = render(&RenderInput {
            derived: &d,
            profile_text: "\r\n첫 줄  \r\n\r\n둘째 줄\r\n\r\n",
            profile_rev: 4,
            human_text: "사람 글\r\n",
            divergence_examples: &[],
        });
        assert!(!out.contains('\r'), "CRLF가 남았다");
        assert!(!out.starts_with('\u{feff}'), "BOM이 붙었다");
        assert!(out.ends_with("<!-- /soul:human -->\n"));
        assert!(!out.ends_with("\n\n"), "파일 끝에 개행은 하나다");
        // 앞뒤 빈 줄은 제거하되 내부 빈 줄은 보존한다 (§8.3 규칙 3과 같은 규칙).
        assert!(
            out.contains("-->\n첫 줄\n\n둘째 줄\n<!-- /soul:neg -->"),
            "{out}"
        );
    }

    #[test]
    fn profile_marker_carries_rev_and_block_hash() {
        let d = spec_derived();
        let out = render(&RenderInput {
            derived: &d,
            profile_text: PROFILE,
            profile_rev: 17,
            human_text: "",
            divergence_examples: &[],
        });
        let marker = out
            .lines()
            .find(|l| l.starts_with("<!-- soul:neg"))
            .expect("profile 마커가 있어야 한다");
        assert_eq!(
            marker,
            format!(
                "<!-- soul:neg id=profile rev=17 hash={} -->",
                block_hash(PROFILE)
            )
        );
        // 마커의 hash는 파일에 실제로 실린 본문의 해시와 같아야 한다 (§8.3 규칙 4).
        assert_eq!(block_hash(PROFILE), block_hash(&block_body(PROFILE)));
    }

    #[test]
    fn skeleton_has_full_structure_with_empty_blocks() {
        // §3 최초 실행 3단계 — 전 블록이 비어 있고 rev=0.
        let s = skeleton();
        assert_eq!(structure_without_neg_open(&s), SPEC_STRUCTURE);
        assert!(s.contains(&format!(
            "<!-- soul:neg id=profile rev=0 hash={} -->\n\n<!-- /soul:neg -->\n",
            block_hash("")
        )));
        assert!(s.contains("<!-- soul:human -->\n\n<!-- /soul:human -->\n"));
        assert!(s.contains("기준 — · 관측 0 · 최초 —\n"));
        assert!(!s.contains("대표 사례"));
        assert!(s.ends_with("<!-- /soul:human -->\n"));
    }

    #[test]
    fn structure_is_identical_whether_values_exist_or_not() {
        let d = spec_derived();
        let full = render(&RenderInput {
            derived: &d,
            profile_text: PROFILE,
            profile_rev: 17,
            human_text: "메모",
            divergence_examples: &ids(),
        });
        assert_eq!(structure_without_neg_open(&full), SPEC_STRUCTURE);
        assert_eq!(
            structure_without_neg_open(&full),
            structure_without_neg_open(&skeleton()),
            "값이 없어도 블록은 그대로 있어야 한다"
        );
    }

    #[test]
    fn change_column_uses_unicode_minus_not_ascii_hyphen() {
        let mut d = spec_derived();
        d.axes_change = [Some(-0.5); 8];
        let out = render(&RenderInput {
            derived: &d,
            profile_text: PROFILE,
            profile_rev: 1,
            human_text: "",
            divergence_examples: &[],
        });
        assert!(out.contains("| chroma | 0.34 | \u{2212}0.50 |"), "{out}");
        assert!(!out.contains("| -0.50 |"), "ASCII 하이픈을 쓰면 안 된다");
    }

    #[test]
    fn rendered_document_parses_back_and_profile_is_not_dirty() {
        // 렌더 → 파싱 왕복. 마커 짝이 맞고(§8.3 규칙 6) 마커의 hash가
        // 실제 본문 해시와 같아야 한다(규칙 4) — 아니면 저장할 때마다 헛 `profile_edit`이 뜬다.
        let d = spec_derived();
        let out = render(&RenderInput {
            derived: &d,
            profile_text: PROFILE,
            profile_rev: 17,
            human_text: "사람이 쓴 줄",
            divergence_examples: &ids(),
        });
        let doc = crate::soulmd::parse::parse(&out).expect("렌더 결과는 항상 파싱된다");
        let profile = doc.block("profile").expect("profile 블록");
        assert_eq!(profile.rev, Some(17));
        assert!(!profile.is_dirty(), "갓 렌더한 문서는 dirty가 아니다");
        assert_eq!(profile.normalized_body(), PROFILE);
        assert_eq!(doc.human_body().map(str::trim), Some("사람이 쓴 줄"));
        assert!(doc.block("header").is_some());
        assert!(doc.block("axes").is_some());
        assert!(doc.block("divergence").is_some());
    }

    #[test]
    fn separator_is_middle_dot_with_spaces() {
        let d = spec_derived();
        let out = render(&RenderInput {
            derived: &d,
            profile_text: PROFILE,
            profile_rev: 1,
            human_text: "",
            divergence_examples: &[],
        });
        let header = out.lines().nth(3).expect("헤더 첫 줄");
        assert_eq!(header.matches(SEP).count(), 2);
        assert!(header.contains('\u{00b7}'));
    }
}
