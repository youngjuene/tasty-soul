//! `soul-media` — 로컬 미디어 처리 (§9.1 · §9.4 · §9.6 · §9.7 · §20.5).
//!
//! **네트워크를 쓰지 않는다.** ffmpeg 조달(다운로드)은 `soul-net`에 있다.
//!
//! 로컬 CPU를 지키는 방법은 필터를 최적화하는 것이 아니라 **처리 구간 자체를 자르는 것**이다
//! (§20.5). `-t 30`을 주면 ffmpeg이 30초 이후를 디코딩하지 않으므로 10분짜리든
//! 2시간짜리든 로컬 비용이 동일하다 (T43·T43b).

pub mod audio;
pub mod ffmpeg;
pub mod image_in;
pub mod probe;
pub mod thumb;
pub mod video;

pub use probe::{detect_kind, DetectedKind};
