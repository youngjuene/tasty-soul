//! 정준 JSON 직렬화 (§R6).
//!
//! > UTF-8, BOM 없음, LF, 들여쓰기 2칸, **키 사전순 정렬**, 부동소수는 소수점
//! > 6자리 반올림 후 저장.
//!
//! `serde_json::to_string_pretty`에 기대지 않고 직접 쓴다. 이유:
//! `serde_json`의 `preserve_order` 기능이 의존성 그래프 어딘가에서 켜지면
//! 키 정렬이 조용히 사라진다 — 크래시하지 않고 T1만 실패한다.

use serde::Serialize;
use serde_json::Value;

/// §R6의 부동소수 반올림 자릿수.
pub const FLOAT_DECIMALS: i32 = 6;

/// `f64`를 소수점 6자리로 반올림한다. 저장·비교 양쪽에서 이 함수만 쓴다.
#[inline]
pub fn round6(v: f64) -> f64 {
    if !v.is_finite() {
        return 0.0;
    }
    let f = 10f64.powi(FLOAT_DECIMALS);
    let r = (v * f).round() / f;
    // -0.0 을 0.0 으로 정규화한다. 그러지 않으면 "-0.0" 이 파일에 남는다.
    if r == 0.0 {
        0.0
    } else {
        r
    }
}

/// 값을 정준 JSON 문자열로 만든다. **끝에 개행 하나를 붙인다.**
pub fn to_string<T: Serialize>(value: &T) -> crate::Result<String> {
    let v = serde_json::to_value(value)?;
    let mut out = String::with_capacity(512);
    write_value(&v, 0, &mut out);
    out.push('\n');
    Ok(out)
}

/// 서명·해시용 압축 형태 (개행·들여쓰기 없음, 키는 여전히 정렬).
pub fn to_compact<T: Serialize>(value: &T) -> crate::Result<String> {
    let v = serde_json::to_value(value)?;
    let mut out = String::with_capacity(256);
    write_compact(&v, &mut out);
    Ok(out)
}

fn write_value(v: &Value, depth: usize, out: &mut String) {
    match v {
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();
            out.push_str("{\n");
            for (i, k) in keys.iter().enumerate() {
                indent(depth + 1, out);
                write_str(k, out);
                out.push_str(": ");
                write_value(&map[k.as_str()], depth + 1, out);
                if i + 1 < keys.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            indent(depth, out);
            out.push('}');
        }
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            for (i, it) in items.iter().enumerate() {
                indent(depth + 1, out);
                write_value(it, depth + 1, out);
                if i + 1 < items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            indent(depth, out);
            out.push(']');
        }
        Value::String(s) => write_str(s, out),
        Value::Number(n) => write_number(n, out),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Null => out.push_str("null"),
    }
}

fn write_compact(v: &Value, out: &mut String) {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_str(k, out);
                out.push(':');
                write_compact(&map[k.as_str()], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, it) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_compact(it, out);
            }
            out.push(']');
        }
        Value::String(s) => write_str(s, out),
        Value::Number(n) => write_number(n, out),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Null => out.push_str("null"),
    }
}

fn write_number(n: &serde_json::Number, out: &mut String) {
    if let Some(u) = n.as_u64() {
        out.push_str(&u.to_string());
    } else if let Some(i) = n.as_i64() {
        out.push_str(&i.to_string());
    } else if let Some(f) = n.as_f64() {
        let r = round6(f);
        if r.fract() == 0.0 && r.abs() < 1e15 {
            // 6자리 반올림 결과가 정수면 `1.0` 형태로 남긴다 (JSON 숫자 타입 보존).
            out.push_str(&format!("{r:.1}"));
        } else {
            // ryu 최단 표현. 6자리로 반올림했으므로 지수 표기가 나오지 않는다.
            out.push_str(&r.to_string());
        }
    } else {
        out.push_str(&n.to_string());
    }
}

fn indent(depth: usize, out: &mut String) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn write_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            // 비ASCII는 이스케이프하지 않는다. 파일은 UTF-8이며 한국어가 읽혀야 한다.
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keys_are_sorted_and_indent_is_two() {
        let v = json!({ "z": 1, "a": { "y": true, "b": null } });
        assert_eq!(
            to_string(&v).unwrap(),
            "{\n  \"a\": {\n    \"b\": null,\n    \"y\": true\n  },\n  \"z\": 1\n}\n"
        );
    }

    #[test]
    fn floats_round_to_six_places() {
        let v = json!({ "x": 0.123456789_f64, "y": 1.0_f64/3.0 });
        let s = to_string(&v).unwrap();
        assert!(s.contains("\"x\": 0.123457"), "{s}");
        assert!(s.contains("\"y\": 0.333333"), "{s}");
    }

    #[test]
    fn whole_floats_keep_decimal_point() {
        let v = json!({ "w": 1.0_f64 });
        assert!(to_string(&v).unwrap().contains("\"w\": 1.0"));
    }

    #[test]
    fn negative_zero_is_normalized() {
        assert_eq!(round6(-0.0000001), 0.0);
        assert!(!to_string(&json!({"a": -0.0000001_f64}))
            .unwrap()
            .contains("-0"));
    }

    #[test]
    fn korean_is_not_escaped() {
        let s = to_string(&json!({"p": "차갑고 정돈된"})).unwrap();
        assert!(s.contains("차갑고 정돈된"));
    }

    #[test]
    fn no_crlf_anywhere() {
        let s = to_string(&json!({"a": [1, 2, {"b": "c"}]})).unwrap();
        assert!(!s.contains('\r'));
    }

    #[test]
    fn empty_containers_are_inline() {
        assert_eq!(
            to_string(&json!({"a": [], "b": {}})).unwrap(),
            "{\n  \"a\": [],\n  \"b\": {}\n}\n"
        );
    }
}
