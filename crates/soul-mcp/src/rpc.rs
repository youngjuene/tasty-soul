//! stdio JSON-RPC 2.0 (MCP 전송 계층).
//!
//! 줄 단위 JSON 메시지를 stdin에서 읽고 stdout에 쓴다.
//! **stdout에 로그를 쓰지 않는다** — 프로토콜 스트림이다. 로그는 stderr로 보낸다.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// JSON-RPC 2.0 표준 에러 코드. MCP는 이 코드를 그대로 쓴다.
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

#[derive(Debug, Deserialize)]
pub struct Request {
    #[allow(dead_code)]
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Serialize)]
pub struct Response {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Debug, Serialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl Response {
    pub fn ok(id: Value, result: Value) -> Response {
        Response {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }
    pub fn err(id: Value, code: i64, message: impl Into<String>) -> Response {
        Response {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
            }),
        }
    }
}

impl Request {
    /// `id`가 없으면 notification이다. **응답을 보내지 않는다.**
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

/// 한 줄을 요청으로 해석한다.
///
/// 실패하면 그대로 돌려보낼 수 있는 에러 응답을 만든다. JSON 자체가 깨졌으면 `-32700`,
/// JSON이지만 JSON-RPC 형태가 아니면 `-32600`이다. 후자는 `id`만이라도 건져
/// 클라이언트가 어느 요청이 실패했는지 알 수 있게 한다.
pub fn parse_line(line: &str) -> std::result::Result<Request, Response> {
    let value: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return Err(Response::err(
                Value::Null,
                PARSE_ERROR,
                format!("JSON 파싱 실패: {e}"),
            ))
        }
    };
    let id = value.get("id").cloned().unwrap_or(Value::Null);
    serde_json::from_value::<Request>(value)
        .map_err(|e| Response::err(id, INVALID_REQUEST, format!("JSON-RPC 요청 형식 오류: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_has_no_id() {
        let r = parse_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).unwrap();
        assert!(r.is_notification());
        assert_eq!(r.params, Value::Null);
    }

    #[test]
    fn broken_json_is_parse_error_with_null_id() {
        let e = parse_line("{not json").unwrap_err();
        assert_eq!(e.error.as_ref().unwrap().code, PARSE_ERROR);
        assert_eq!(e.id, Value::Null);
    }

    #[test]
    fn valid_json_wrong_shape_keeps_the_id() {
        // method가 없으면 요청이 아니다. 그래도 id는 돌려준다.
        let e = parse_line(r#"{"jsonrpc":"2.0","id":7}"#).unwrap_err();
        assert_eq!(e.error.as_ref().unwrap().code, INVALID_REQUEST);
        assert_eq!(e.id, serde_json::json!(7));
    }

    #[test]
    fn response_omits_the_unused_half() {
        let ok = serde_json::to_string(&Response::ok(serde_json::json!(1), Value::Null)).unwrap();
        assert!(!ok.contains("error"), "{ok}");
        let err = serde_json::to_string(&Response::err(serde_json::json!(1), -32601, "x")).unwrap();
        assert!(!err.contains("result"), "{err}");
    }
}
