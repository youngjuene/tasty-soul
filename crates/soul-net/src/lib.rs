//! `soul-net` — 원격 호출 전부 (§D1의 "구축" 국면).
//!
//! **`soul-core`와 `soul-mcp`는 이 크레이트를 의존하지 않는다.** 그것이 §19.4의
//! "적용은 로컬" 약속을 구조적으로 보장하는 방법이다.
//!
//! 여기서 나가는 것은 §D2 표에 적힌 것이 전부여야 한다. 표에 없는 것을 보내지 않는다.

pub mod embed;
pub mod openai;
pub mod procure;
pub mod search;
pub mod secrets;
pub mod youtube;

pub use openai::OpenAi;
