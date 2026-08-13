//! `soul-core` — tasty-soul 도메인 핵심.
//!
//! **이 크레이트는 네트워크를 쓰지 않는다.** `reqwest`/`hyper` 등 HTTP 클라이언트를
//! 의존성에 추가하지 말 것 — `soul-mcp`가 이 크레이트만 링크하며, 그것이 §19.4의
//! "적용 국면은 기기를 떠나지 않는다"를 구조적으로 보장한다 (T32·T34).
//!
//! 명세: `tasty-soul-mvp-plan.md`

pub mod canon;
pub mod config;
pub mod db;
pub mod derived;
pub mod error;
pub mod git;
pub mod ids;
pub mod lock;
pub mod obs;
pub mod paths;
pub mod prompts;
pub mod rebuild;
pub mod soulblocks;
pub mod soulmd;
pub mod time;
pub mod trace;
pub mod vecmath;

pub use error::{Result, SoulError};
pub use ids::ObsId;
pub use time::Ts;

/// 관측 스키마 버전 (§6.1).
pub const SCHEMA_VERSION: u32 = 1;

/// git 커밋 작성자 (§R8). 고정값이다.
pub const GIT_AUTHOR_NAME: &str = "tasty-soul";
pub const GIT_AUTHOR_EMAIL: &str = "noreply@localhost";
