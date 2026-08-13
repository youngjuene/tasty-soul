//! `soul-pipeline` — 수집 오케스트레이션 (§9).
//!
//! ```text
//! 입력 → kind 판별 → YouTube면 §9.3 해석 → sha256 계산 → 썸네일 생성
//!      → (모달리티별 서술 생성) → machine.prose 확정
//!      → embed(machine.prose) → min_dist·surprisal 계산 → ingest 관측 기록
//!      ├─ 즉시 → 화면 2 (감각 글귀 ○/×)
//!      └─ 큐 등록 → §9.10 비평 작업 → context 관측 → 화면 2.1 (문화 글귀 ○/×)
//! ```
//!
//! **두 갈래는 독립적으로 진행된다.** 비평 실패가 투입 자체를 실패시키지 않는다 (T51).
//!
//! **`ingest`나 `reading`은 `SOUL.md` 재렌더를 유발하지 않는다** (§R8, T21b).

pub mod app;
pub mod critique_worker;
pub mod describe;
pub mod doctor;
pub mod ingest;
pub mod reading;
pub mod recast;
pub mod reflect_flow;

pub use app::App;
