//! 단일 에러 타입 (§15).

use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, SoulError>;

#[derive(Debug, thiserror::Error)]
pub enum SoulError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("toml 읽기: {0}")]
    TomlDe(#[from] toml::de::Error),

    #[error("toml 쓰기: {0}")]
    TomlSer(#[from] toml::ser::Error),

    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("git: {0}")]
    Git(#[from] git2::Error),

    /// §R7 — 쓰기 락 획득 실패. **대기하지 않고 즉시 에러다.**
    #[error("쓰기 락을 얻지 못했습니다 (pid {holder:?}). 다른 인스턴스가 실행 중입니다")]
    LockBusy { holder: Option<u32> },

    /// §8.3 규칙 6 — 마커 짝 불일치. 파일을 쓰지 않는다.
    #[error("SOUL.md 파싱 실패 ({line}행): {reason}")]
    SoulParse { line: usize, reason: String },

    /// §R3 — `--offline`인데 임베딩 캐시 미스.
    #[error("오프라인 재빌드 중 임베딩 캐시 미스: {0}")]
    EmbedCacheMiss(String),

    #[error("관측 {0} 을 찾을 수 없습니다")]
    ObsNotFound(String),

    #[error("잘못된 관측 ID: {0}")]
    BadId(String),

    #[error("설정 오류: {0}")]
    Config(String),

    #[error("경로 없음: {0}")]
    MissingPath(PathBuf),

    /// §11.2 — `propose_delta` 가드레일 위반. 사유가 툴 결과로 에이전트에 돌아간다.
    #[error("{0}")]
    Guardrail(String),

    #[error("{0}")]
    Invalid(String),
}

impl SoulError {
    pub fn invalid(msg: impl Into<String>) -> Self {
        SoulError::Invalid(msg.into())
    }
    pub fn guardrail(msg: impl Into<String>) -> Self {
        SoulError::Guardrail(msg.into())
    }
    pub fn config(msg: impl Into<String>) -> Self {
        SoulError::Config(msg.into())
    }
}
