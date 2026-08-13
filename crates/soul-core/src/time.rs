//! 시간 (§R1).
//!
//! **모든 파생값의 시간 기준점은 `T_ref` = 전체 관측 중 가장 늦은 `ts`다.**
//! `now()`는 (a) 새 관측을 기록할 때 (b) git 커밋 타임스탬프에만 쓴다.

use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// 관측 타임스탬프. 직렬화 형태는 **정확히** `2026-08-13T09:12:33.123Z` 다.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ts(DateTime<Utc>);

pub const TS_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.3fZ";

impl Ts {
    /// 새 관측 기록 시점의 벽시계 (§R1이 허용하는 두 용처 중 하나).
    pub fn now() -> Self {
        Ts(Utc::now())
    }

    pub fn from_datetime(dt: DateTime<Utc>) -> Self {
        Ts(dt)
    }

    pub fn as_datetime(&self) -> DateTime<Utc> {
        self.0
    }

    pub fn parse(s: &str) -> Option<Self> {
        DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| Ts(d.with_timezone(&Utc)))
    }

    /// `YYYY-MM-DD`, UTC 기준 (§8.2.1).
    pub fn date_string(&self) -> String {
        self.0.format("%Y-%m-%d").to_string()
    }

    /// 월별 샤딩 디렉토리 이름 `YYYY-MM` (§20.6).
    pub fn month(&self) -> Month {
        Month {
            year: self.0.year(),
            month: self.0.month(),
        }
    }

    /// `self - days` (일 단위 창 경계).
    pub fn minus_days(&self, days: i64) -> Ts {
        Ts(self.0 - chrono::Duration::days(days))
    }

    /// 경과 일수 (실수). 축 감쇠 `0.5^(Δ/30d)` 계산에 쓴다 (§12.1).
    pub fn days_since(&self, earlier: Ts) -> f64 {
        let secs = (self.0 - earlier.0).num_milliseconds() as f64 / 1000.0;
        secs / 86_400.0
    }

    pub fn to_rfc3339_millis(&self) -> String {
        self.0.format(TS_FORMAT).to_string()
    }
}

impl fmt::Debug for Ts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_rfc3339_millis())
    }
}

impl fmt::Display for Ts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_rfc3339_millis())
    }
}

impl Serialize for Ts {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_rfc3339_millis())
    }
}

impl<'de> Deserialize<'de> for Ts {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ts::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("잘못된 타임스탬프: {s}")))
    }
}

/// 달력 월 버킷 (§12.5). **상대 오프셋을 쓰지 않는다** — 과거 달의 경계가 움직이면
/// 관측 1건 추가로 전체 캐시가 무효화된다.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Month {
    pub year: i32,
    pub month: u32,
}

impl Month {
    pub fn new(year: i32, month: u32) -> Self {
        Month { year, month }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let (y, m) = s.split_once('-')?;
        Some(Month {
            year: y.parse().ok()?,
            month: m.parse().ok()?,
        })
    }

    /// UTC 월의 첫 순간 (포함).
    pub fn start(&self) -> Ts {
        Ts(Utc.from_utc_datetime(
            &NaiveDate::from_ymd_opt(self.year, self.month, 1)
                .expect("유효한 월")
                .and_hms_opt(0, 0, 0)
                .unwrap(),
        ))
    }

    /// 다음 달의 첫 순간 (미포함 상한).
    pub fn end_exclusive(&self) -> Ts {
        self.next().start()
    }

    pub fn next(&self) -> Month {
        if self.month == 12 {
            Month::new(self.year + 1, 1)
        } else {
            Month::new(self.year, self.month + 1)
        }
    }

    pub fn prev(&self) -> Month {
        if self.month == 1 {
            Month::new(self.year - 1, 12)
        } else {
            Month::new(self.year, self.month - 1)
        }
    }

    pub fn contains(&self, ts: Ts) -> bool {
        ts.month() == *self
    }

    /// `self ..= to` 오름차순 월 목록 (§12.5의 `M`).
    pub fn range_through(&self, to: Month) -> Vec<Month> {
        let mut out = Vec::new();
        let mut cur = *self;
        while cur <= to {
            out.push(cur);
            cur = cur.next();
        }
        out
    }
}

impl fmt::Display for Month {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}", self.year, self.month)
    }
}

impl Serialize for Month {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Month {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Month::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("잘못된 월: {s}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ts_roundtrip_is_millisecond_z() {
        let t = Ts::parse("2026-08-13T09:12:33.123Z").unwrap();
        assert_eq!(t.to_rfc3339_millis(), "2026-08-13T09:12:33.123Z");
        assert_eq!(t.date_string(), "2026-08-13");
        assert_eq!(t.month().to_string(), "2026-08");
    }

    #[test]
    fn month_range_crosses_year() {
        let a = Month::new(2025, 11);
        let b = Month::new(2026, 2);
        let r = a.range_through(b);
        assert_eq!(
            r.iter().map(|m| m.to_string()).collect::<Vec<_>>(),
            ["2025-11", "2025-12", "2026-01", "2026-02"]
        );
    }

    #[test]
    fn days_since_is_fractional() {
        let a = Ts::parse("2026-08-01T00:00:00.000Z").unwrap();
        let b = Ts::parse("2026-08-31T12:00:00.000Z").unwrap();
        assert!((b.days_since(a) - 30.5).abs() < 1e-9);
    }
}
