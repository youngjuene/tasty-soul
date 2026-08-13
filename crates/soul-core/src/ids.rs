//! 관측 ID (§6).
//!
//! **ULID의 사전순 = 시간순 = 재생 순서.** 이 성질에 모든 재빌드가 의존하므로
//! ID는 항상 26자 Crockford base32 대문자로 저장한다.

use crate::error::{Result, SoulError};
use crate::time::Ts;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Mutex;

/// 관측 ID. 내부 표현은 정규화된 26자 문자열이다.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ObsId(String);

impl ObsId {
    /// 문자열에서 파싱한다. 26자·Crockford base32가 아니면 거부한다.
    pub fn parse(s: &str) -> Result<Self> {
        let u = ulid::Ulid::from_string(s).map_err(|_| SoulError::BadId(s.to_string()))?;
        Ok(ObsId(u.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn to_ulid(&self) -> ulid::Ulid {
        ulid::Ulid::from_string(&self.0).expect("ObsId는 항상 유효한 ULID다")
    }

    /// ULID에 인코딩된 시각. `Ts`와 반드시 일치하지는 않으므로 정렬 외에는 쓰지 않는다.
    pub fn timestamp(&self) -> Ts {
        let ms = self.to_ulid().timestamp_ms();
        Ts::from_datetime(
            chrono::DateTime::from_timestamp_millis(ms as i64).unwrap_or_else(chrono::Utc::now),
        )
    }
}

impl fmt::Debug for ObsId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for ObsId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for ObsId {
    type Err = SoulError;
    fn from_str(s: &str) -> Result<Self> {
        ObsId::parse(s)
    }
}

static GEN: Mutex<Option<ulid::Generator>> = Mutex::new(None);

/// 단조 증가 ULID를 만든다.
///
/// 같은 밀리초 안에서 여러 관측이 생겨도 사전순이 생성순과 일치해야 한다
/// (§8.4의 한 번 저장 = 커밋 여러 개 시나리오). 표준 `Ulid::generate`는
/// 그것을 보장하지 않으므로 단조 생성기를 쓴다.
pub fn new_id() -> ObsId {
    let mut guard = GEN.lock().unwrap_or_else(|e| e.into_inner());
    let g = guard.get_or_insert_with(ulid::Generator::new);
    let u = match g.generate() {
        Ok(u) => u,
        // 같은 ms 안에서 랜덤 성분이 소진된 극단적인 경우: 다음 ms를 기다린다.
        Err(_) => {
            std::thread::sleep(std::time::Duration::from_millis(1));
            g.generate().unwrap_or_else(|_| ulid::Ulid::generate())
        }
    };
    ObsId(u.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_ids_are_monotonic_and_sortable() {
        let ids: Vec<ObsId> = (0..500).map(|_| new_id()).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted, "생성순과 사전순이 일치해야 한다");
        assert!(ids.iter().all(|i| i.as_str().len() == 26));
    }

    #[test]
    fn rejects_garbage() {
        assert!(ObsId::parse("not-a-ulid").is_err());
        assert!(ObsId::parse("01J8XQZK3M7P4RSTVWXYZ").is_err()); // 21자
    }
}
