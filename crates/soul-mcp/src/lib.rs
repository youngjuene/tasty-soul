//! `soul-mcp` — 로컬 MCP 서버의 본체 (§19).
//!
//! 바이너리(`main.rs`)는 [`server::run_stdio`]를 부르기만 한다. 로직이 여기 lib에 있어야
//! `crates/soul-mcp/tests/`의 인수 테스트(T30–T38)가 stdin/stdout 없이 서버를 부를 수 있다.
//!
//! ## 절대 규칙
//!
//! **이 크레이트에 HTTP 클라이언트를 링크하지 않는다** (§19.4, T32). 의존성은
//! `soul-core` + `serde` + `serde_json` 뿐이며, MCP 프로토콜은 직접 구현한다.
//! **쓰기 툴을 제공하지 않는다** (§19.6, T34).

pub mod resources;
pub mod rpc;
pub mod server;
pub mod tools;
