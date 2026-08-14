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
            // `-0.0 < 0.0` 은 IEEE754 에서 **거짓**이다. 그래서 -0.001 은 else 로 떨어지고
            // `format!("+{:.2}", -0.0)` 이 `"+-0.00"` 이라는 망가진 문자열을 만들었다.
            // 0으로 반올림되면 부호는 의미가 없으므로 `+0.00` 으로 못박는다.
            if r == 0.0 {
                "+0.00".to_string()
            } else if r < 0.0 {
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
        Some(x) => {
            // 소수 둘째 자리에서 0이 되면 **부호를 떼고** `0.00` 으로 적는다.
            //
            // `format!("{:.2}")` 는 -0.002882 를 `-0.00` 으로 만든다. 읽는 사람에게는
            // 서식 오류처럼 보이고, 실제로는 0에 가까운 값인데 "음수"라는 인상을 준다.
            // 실루엣 계수(§12.5의 `crystal`)는 음수가 될 수 있어서 실제로 나타난다.
            // `canon::round6` 이 JSON 에서 같은 정규화를 하는 것과 같은 취지다.
            let r = (x * 100.0).round() / 100.0;
            if r == 0.0 {
                "0.00".to_string()
            } else {
                format!("{r:.2}")
            }
        }
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
        assert_eq!(
            fmt_value(Some(-0.42)),
            "-0.42",
            "진짜 음수는 부호를 유지한다"
        );
    }

    /// 0으로 반올림되는 음수가 `-0.00` 으로 새지 않는다.
    ///
    /// 실루엣 계수는 음수가 될 수 있어서(§12.5) 실제 데이터에서 나타났다.
    /// `해상도 -0.00` 은 서식 오류처럼 보인다.
    #[test]
    fn value_never_renders_negative_zero() {
        assert_eq!(fmt_value(Some(-0.002882)), "0.00");
        assert_eq!(fmt_value(Some(-0.0)), "0.00");
        assert_eq!(fmt_value(Some(-0.004)), "0.00");
        assert_eq!(fmt_value(Some(0.0)), "0.00");
        // 반올림해도 0이 아니면 그대로 음수다.
        assert_eq!(fmt_value(Some(-0.006)), "-0.01");
    }

    /// `fmt_change` 는 부호를 일부러 붙이지만 `−0.00` 을 내지 않는다.
    #[test]
    fn change_never_renders_negative_zero() {
        assert_eq!(fmt_change(Some(-0.001)), "+0.00");
        assert_eq!(fmt_change(Some(-0.0)), "+0.00");
    }
}
