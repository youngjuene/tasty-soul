//! `soul-agent` — 에이전트 루프 (§11).
//!
//! Hermes 스타일 툴콜 루프 — **명시적 스키마, 턴 상한, 종결 툴.**
//! OpenAI 네이티브 function calling으로 구현하되 루프 규율은 동일하게 유지한다.
//!
//! 수집 파이프라인은 전부 결정론적이다(§9). YouTube 해석도 고정 순서 핸들러이므로
//! 에이전트가 아니다. **에이전트는 아래 두 곳에만 존재한다.**

pub mod critique;
pub mod reflect;
pub mod schemas;
