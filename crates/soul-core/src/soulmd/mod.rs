//! `SOUL.md` — 포맷·파서·렌더러·저장 시퀀스 (§8).

pub mod parse;
pub mod render;
pub mod save;

pub use parse::{block_hash, normalize_for_hash, parse, BlockKind, SoulBlock, SoulDoc};
pub use render::{render, RenderInput};
pub use save::{save_edited, SaveOutcome};

/// §R10 — 파생값이 `null`이면 `—`(em dash) 하나로 렌더한다.
/// **0으로 대체하거나 항목을 생략하지 않는다.**
pub const NULL_GLYPH: &str = "—";

/// §8.2 템플릿이 쓰는 구분자 (U+00B7 MIDDLE DOT).
pub const SEP: &str = " · ";

/// 음수 부호는 U+2212 MINUS SIGN이다. ASCII 하이픈이 아니다 (§8.2 템플릿).
pub const MINUS: char = '\u{2212}';

/// `+0.03` / `−0.06` / `—` 형식 (§8.2.1).
pub fn fmt_change(v: Option<f64>) -> String {
    match v {
        None => NULL_GLYPH.to_string(),
        Some(x) => {
            let r = (x * 100.0).round() / 100.0;
            if r < 0.0 {
                format!("{}{:.2}", MINUS, -r)
            } else {
                format!("+{r:.2}")
            }
        }
    }
}

/// `0.34` / `—` 형식 (§8.2.1 — 소수 둘째 자리).
pub fn fmt_value(v: Option<f64>) -> String {
    match v {
        None => NULL_GLYPH.to_string(),
        Some(x) => format!("{x:.2}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn change_uses_unicode_minus_and_explicit_plus() {
        assert_eq!(fmt_change(Some(0.03)), "+0.03");
        assert_eq!(fmt_change(Some(-0.06)), "\u{2212}0.06");
        assert_eq!(fmt_change(None), "—");
        // 부호가 있는 0은 +0.00 이다 (변화 없음이 아니라 측정된 0).
        assert_eq!(fmt_change(Some(0.0)), "+0.00");
    }

    #[test]
    fn value_is_two_decimals() {
        assert_eq!(fmt_value(Some(0.3)), "0.30");
        assert_eq!(fmt_value(None), "—");
    }
}
