//! `soul-cli` — 명령 표면 (§14).
//!
//! 통합 테스트(`tests/`)는 바이너리 크레이트의 모듈을 볼 수 없다. §14의 모든 명령이
//! 파싱되는지(`Cli::try_parse_from`)와 T38을 `crates/soul-cli/tests/`에서 검증하려면
//! lib 타깃이 있어야 한다. `src/main.rs`는 이 lib의 [`cmd::run`]을 부르는 껍데기다.

pub mod cmd;
