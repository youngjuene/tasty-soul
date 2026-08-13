//! MCP 서버 루프 (§19.2 · §19.7).
//!
//! ## 서버 인스트럭션 (§19.2.1 · §19.5)
//!
//! §19.5의 검색 패턴은 툴 설명이 아니라 **서버 인스트럭션에 한 번만** 적는다.
//! 툴 설명문마다 반복하면 스키마 예산을 낭비한다.
//!
//! > 닻이 될 항목을 `soul_recall`의 필터로 찾고, `soul_similar`로 그 주변을 넓힌다.

use crate::rpc::{self, Request, Response};
use serde_json::{json, Value};
use soul_core::config::Config;
use soul_core::db::embed_cache::cache_key;
use soul_core::db::Db;
use soul_core::derived::{Derived, EmbeddingLookup};
use soul_core::obs::Store;
use soul_core::paths::Paths;
use soul_core::rebuild::EMBED_PROVIDER;
use std::io::{BufRead, Write};

pub const SERVER_NAME: &str = "soul";
pub const PROTOCOL_VERSION: &str = "2025-06-18";
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 클라이언트가 매 턴 컨텍스트에 싣는 문자열이다. 짧게 유지한다 (§19.2.1).
pub const INSTRUCTIONS: &str = "\
이 서버는 한 사람의 취향 아카이브를 읽기 전용으로 제공한다. 쓰기 툴은 없다.
soul://md 는 취향 문서 원문이다. 먼저 읽어라.
자유 텍스트 의미 검색은 없다. 대신 soul_recall 의 태그·종류·기간 필터로 닻이 될 관측을 찾고,
그 id 로 soul_similar 를 불러 주변을 넓혀라. 근거가 필요하면 soul_read 로 원본까지 내려가라.
soul_readings 는 사용자 본인이 쓴 문장이며 cell=other_reason 이 가장 강한 신호다.";

/// 서버 컨텍스트. `soul/`을 읽기 전용으로 연다 (§19.6, T35).
pub struct Ctx {
    pub paths: soul_core::paths::Paths,
    pub config: soul_core::config::Config,
    /// `derived.sqlite`가 없으면 `None`. `soul_similar` 호출 시 **명확한 에러**를 낸다 (T36).
    pub db: Option<soul_core::db::Db>,
    pub set: soul_core::obs::ObsSet,
}

impl Ctx {
    /// 시작 시 한 번 부른다. 관측 로그 전체를 메모리에 올린다.
    ///
    /// **어떤 파일도 쓰지 않는다.** `derived.sqlite`는 `open_read_only`로 열고,
    /// 열리지 않으면 사유를 stderr에 남긴 뒤 `None`으로 계속한다 — 캐시가 없다고
    /// 서버 전체가 죽으면 `soul://md`조차 읽을 수 없다 (T35·T36).
    pub fn load(paths: Paths) -> anyhow::Result<Ctx> {
        let config = Config::load(&paths.config_toml())?;
        let set = Store::new(paths.clone()).load_set()?;
        let db = match Db::open_read_only(&paths.derived_db()) {
            Ok(db) => Some(db),
            Err(e) => {
                eprintln!(
                    "[soul-mcp] derived.sqlite 를 열지 못했습니다 ({e}). 유사도 툴이 막힙니다"
                );
                None
            }
        };
        Ok(Ctx {
            paths,
            config,
            db,
            set,
        })
    }

    /// `SOUL_ROOT` 또는 OS 기본 경로에서 연다 (§3).
    pub fn discover() -> anyhow::Result<Ctx> {
        Ctx::load(Paths::discover()?)
    }

    /// 캐시된 임베딩 조회기. **네트워크를 쓰지 않는다** — 미스는 그냥 `None`이다 (T33).
    pub fn embeds(&self) -> DbEmbeds<'_> {
        DbEmbeds {
            db: self.db.as_ref(),
            model: &self.config.embed.model,
            dims: self.config.embed.dims,
        }
    }

    /// §12 파생값 전체. 임베딩이 없으면 군집·drift 계열이 `None`으로 내려앉을 뿐
    /// 축·셀 계산은 그대로 나온다.
    pub fn derived(&self) -> Derived {
        soul_core::derived::compute(
            &self.set,
            &self.embeds(),
            self.config.local.silhouette_max_samples,
        )
    }

    /// T36 — 캐시가 없을 때 **무엇이 없고 무엇을 하면 되는지** 말한다. 크래시하지 않는다.
    pub fn db_or_err(&self) -> anyhow::Result<&Db> {
        if let Some(db) = &self.db {
            return Ok(db);
        }
        let path = self.paths.derived_db();
        // 여기서 다시 열어 sqlite의 구체적인 사유를 얻는다. "없다"와 "스키마가 다르다"는
        // 사용자가 해야 할 일이 다르다.
        let reason = match Db::open_read_only(&path) {
            Ok(_) => "일시적인 오류".to_string(),
            Err(e) => e.to_string(),
        };
        anyhow::bail!(
            "임베딩 캐시를 열 수 없어 유사도를 계산할 수 없습니다: {reason} \
             (경로: {}). `soul rebuild` 로 캐시를 만든 뒤 다시 시도하십시오",
            path.display()
        )
    }
}

/// `derived.sqlite`의 임베딩 캐시를 텍스트로 조회한다 (§R3의 캐시 키).
///
/// `obs_vec`/`critique_vec`이 아니라 `embed_cache`를 본다 — 두 공간(§12.7)의 텍스트가
/// 모두 여기에 있으므로 조회기 하나로 감각·문화 양쪽 coherence가 계산된다.
pub struct DbEmbeds<'a> {
    db: Option<&'a Db>,
    model: &'a str,
    dims: usize,
}

impl EmbeddingLookup for DbEmbeds<'_> {
    fn get(&self, text: &str) -> Option<Vec<f32>> {
        let db = self.db?;
        let key = cache_key(EMBED_PROVIDER, self.model, self.dims, text);
        // 조회 실패(손상된 블롭 등)는 미스와 같이 다룬다. 파생값이 조금 비는 것이
        // 서버가 죽는 것보다 낫다.
        db.embed_get(&key).ok().flatten()
    }
}

/// 요청 하나를 처리한다. **stdin/stdout을 모르므로 그대로 테스트할 수 있다.**
///
/// notification(=`id` 없음)이면 `None`이다. JSON-RPC 2.0은 notification에
/// 응답하지 않는다 — 알 수 없는 메서드여도 마찬가지다.
pub fn handle(ctx: &Ctx, req: &Request) -> Option<Response> {
    let id = req.id.clone()?;
    Some(match req.method.as_str() {
        "initialize" => Response::ok(id, initialize_result(&req.params)),
        "ping" => Response::ok(id, json!({})),
        "resources/list" => Response::ok(id, crate::resources::list()),
        "resources/read" => match resources_read(ctx, &req.params) {
            Ok(v) => Response::ok(id, v),
            Err((code, msg)) => Response::err(id, code, msg),
        },
        "tools/list" => Response::ok(id, crate::tools::list()),
        "tools/call" => match tools_call(ctx, &req.params) {
            Ok(v) => Response::ok(id, v),
            Err((code, msg)) => Response::err(id, code, msg),
        },
        m => Response::err(id, rpc::METHOD_NOT_FOUND, format!("알 수 없는 메서드: {m}")),
    })
}

/// 클라이언트가 보낸 `protocolVersion`을 그대로 돌려준다. 없으면 우리 기본값.
fn initialize_result(params: &Value) -> Value {
    let pv = params
        .get("protocolVersion")
        .and_then(|v| v.as_str())
        .unwrap_or(PROTOCOL_VERSION);
    json!({
        "protocolVersion": pv,
        // 빈 객체가 "이 기능이 있다"는 선언이다. 구독·알림은 지원하지 않으므로 비운다.
        "capabilities": { "resources": {}, "tools": {} },
        "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
        "instructions": INSTRUCTIONS,
    })
}

fn resources_read(ctx: &Ctx, params: &Value) -> std::result::Result<Value, (i64, String)> {
    let uri = params
        .get("uri")
        .and_then(|v| v.as_str())
        .ok_or_else(|| (rpc::INVALID_PARAMS, "uri 가 필요합니다".to_string()))?;
    if !crate::resources::URIS.contains(&uri) {
        return Err((
            rpc::INVALID_PARAMS,
            format!(
                "알 수 없는 리소스: {uri} (가능: {})",
                crate::resources::URIS.join(", ")
            ),
        ));
    }
    crate::resources::read_with(ctx, uri).map_err(|e| (rpc::INTERNAL_ERROR, e.to_string()))
}

/// 툴 실행 실패는 프로토콜 에러가 아니라 `isError: true` 결과로 돌려준다.
/// 그래야 모델이 사유를 읽고 다음 수를 고를 수 있다 (T36).
fn tools_call(ctx: &Ctx, params: &Value) -> std::result::Result<Value, (i64, String)> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| (rpc::INVALID_PARAMS, "name 이 필요합니다".to_string()))?;
    if !crate::tools::NAMES.contains(&name) {
        return Err((
            rpc::INVALID_PARAMS,
            format!(
                "알 수 없는 툴: {name} (가능: {})",
                crate::tools::NAMES.join(", ")
            ),
        ));
    }
    let args = params.get("arguments").cloned().unwrap_or(Value::Null);
    Ok(match crate::tools::call(ctx, name, &args) {
        Ok(v) => tool_result(
            &serde_json::to_string(&v).unwrap_or_else(|e| e.to_string()),
            false,
        ),
        Err(e) => tool_result(&format!("{e:#}"), true),
    })
}

fn tool_result(text: &str, is_error: bool) -> Value {
    json!({
        "content": [ { "type": "text", "text": text } ],
        "isError": is_error,
    })
}

pub fn run_stdio() -> anyhow::Result<()> {
    let ctx = Ctx::discover()?;
    // **로그는 전부 stderr다.** stdout 한 줄이라도 새면 프로토콜이 깨진다.
    eprintln!(
        "[soul-mcp] {} · 관측 {}건 · 읽기 전용",
        ctx.paths.soul().display(),
        ctx.set.len()
    );

    let mut input = std::io::stdin().lock();
    let mut out = std::io::stdout().lock();
    let mut line = String::new();
    loop {
        line.clear();
        // read_line은 EOF에서 0을 준다. lines()와 달리 버퍼를 재사용한다.
        if input.read_line(&mut line)? == 0 {
            break;
        }
        if line.trim().is_empty() {
            continue;
        }
        let response = match rpc::parse_line(&line) {
            Ok(req) => handle(&ctx, &req),
            Err(resp) => Some(resp),
        };
        let Some(response) = response else { continue };
        let mut s = serde_json::to_string(&response)?;
        s.push('\n');
        out.write_all(s.as_bytes())?;
        out.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(method: &str, params: Value) -> Request {
        serde_json::from_value(json!({
            "jsonrpc": "2.0", "id": 1, "method": method, "params": params
        }))
        .expect("요청")
    }

    fn ctx() -> Ctx {
        let root = std::env::temp_dir().join(format!(
            "tasty-soul-mcp-server-{}-{}",
            std::process::id(),
            soul_core::ids::new_id()
        ));
        let paths = Paths::at(&root);
        paths.ensure_dirs().expect("디렉토리");
        Ctx::load(paths).expect("컨텍스트")
    }

    #[test]
    fn initialize_echoes_the_client_protocol_version() {
        let c = ctx();
        let r = handle(
            &c,
            &req("initialize", json!({"protocolVersion": "2024-11-05"})),
        )
        .unwrap();
        let v = r.result.unwrap();
        assert_eq!(v["protocolVersion"], "2024-11-05");
        assert_eq!(v["serverInfo"]["name"], SERVER_NAME);
        assert!(v["capabilities"]["resources"].is_object());
        assert!(v["capabilities"]["tools"].is_object());
        // §19.5의 검색 패턴은 여기에만 있다.
        let instructions = v["instructions"].as_str().unwrap();
        assert!(instructions.contains("soul_similar"), "{instructions}");
    }

    #[test]
    fn initialize_without_version_falls_back_to_ours() {
        let c = ctx();
        let r = handle(&c, &req("initialize", Value::Null)).unwrap();
        assert_eq!(r.result.unwrap()["protocolVersion"], PROTOCOL_VERSION);
    }

    #[test]
    fn notifications_get_no_response() {
        let c = ctx();
        let n: Request =
            serde_json::from_value(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
                .unwrap();
        assert!(handle(&c, &n).is_none());
        // 알 수 없는 메서드여도 notification이면 침묵한다.
        let n: Request =
            serde_json::from_value(json!({"jsonrpc":"2.0","method":"없는것"})).unwrap();
        assert!(handle(&c, &n).is_none());
    }

    #[test]
    fn unknown_method_is_32601_and_ping_is_empty() {
        let c = ctx();
        let r = handle(&c, &req("없는/메서드", Value::Null)).unwrap();
        assert_eq!(r.error.unwrap().code, rpc::METHOD_NOT_FOUND);
        let r = handle(&c, &req("ping", Value::Null)).unwrap();
        assert_eq!(r.result.unwrap(), json!({}));
    }

    #[test]
    fn unknown_resource_and_tool_are_invalid_params() {
        let c = ctx();
        let r = handle(&c, &req("resources/read", json!({"uri": "soul://없음"}))).unwrap();
        assert_eq!(r.error.unwrap().code, rpc::INVALID_PARAMS);
        let r = handle(&c, &req("tools/call", json!({"name": "soul_write"}))).unwrap();
        let e = r.error.unwrap();
        assert_eq!(e.code, rpc::INVALID_PARAMS);
        assert!(e.message.contains("soul_write"), "{}", e.message);
    }

    #[test]
    fn tool_failure_comes_back_as_is_error_not_a_protocol_error() {
        // T36 — 크래시도 프로토콜 에러도 아니고, 모델이 읽을 수 있는 결과다.
        let c = ctx();
        let r = handle(
            &c,
            &req(
                "tools/call",
                json!({"name":"soul_read","arguments":{"observation_id":"엉터리"}}),
            ),
        )
        .unwrap();
        assert!(r.error.is_none());
        let v = r.result.unwrap();
        assert_eq!(v["isError"], true);
        assert_eq!(v["content"][0]["type"], "text");
    }

    #[test]
    fn embeds_without_db_is_a_miss_not_a_panic() {
        let c = ctx();
        assert!(c.db.is_none());
        assert_eq!(c.embeds().get("아무 텍스트"), None);
        // 파생값도 빈 관측 집합에서 그대로 계산된다.
        assert_eq!(c.derived().observation_count, 0);
    }
}
