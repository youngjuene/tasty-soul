//! `cache/derived.sqlite` (§12 · §20.3 · §9.10).
//!
//! **삭제해도 무방한 캐시다** (§3). 다만 임베딩 캐시 테이블만은 삭제 시
//! 재임베딩(네트워크)이 필요하다 (§R3, T2/T2b).

pub mod derived_cache;
pub mod embed_cache;
pub mod queue;
pub mod schema;

pub use schema::Db;
