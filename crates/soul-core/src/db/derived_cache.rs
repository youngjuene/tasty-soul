//! 파생값 캐시 (§12.3 · §12.5 · §20.7).
//!
//! ## 캐시 무효화 규칙
//!
//! - `month_state`: **관측이 추가된 달 하나만** 무효화한다. 새 관측은 언제나 현재 달에만
//!   들어가므로 지난 달의 상태는 한 번 계산하면 영구히 유효하다 (T22·T40).
//! - `cluster_cache`: 캐시 생성 시점보다 관측이 **10% 또는 20건 이상** 늘었을 때만
//!   재계산한다 (§12.3, T46).
//! - `pca_cache`: **`T_ref`의 날짜가 바뀔 때만** 무효화한다 (§13 화면 6).

use crate::derived::cluster::Clustering;
use crate::derived::MonthState;
use crate::error::{Result, SoulError};
use crate::time::{Month, Ts};
use crate::vecmath::{from_f16_blob, to_f16_blob};
use rusqlite::OptionalExtension;

impl super::Db {
    pub fn month_state_get(&self, month: Month) -> Result<Option<MonthState>> {
        let row: Option<(Option<f64>, Option<f64>, i64)> = self
            .conn
            .query_row(
                "SELECT drift, crystal, n FROM month_state WHERE month = ?1",
                rusqlite::params![month.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        Ok(row.map(|(drift, crystal, n)| MonthState {
            month,
            drift,
            crystal,
            n: n.max(0) as usize,
        }))
    }

    pub fn month_state_put(&self, s: &MonthState) -> Result<()> {
        self.conn.execute(
            "INSERT INTO month_state(month, drift, crystal, n) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(month) DO UPDATE SET
                 drift = excluded.drift, crystal = excluded.crystal, n = excluded.n",
            rusqlite::params![s.month.to_string(), s.drift, s.crystal, s.n as i64],
        )?;
        Ok(())
    }

    /// 그 달 하나만 지운다. **다른 달을 건드리지 않는다** (T40).
    pub fn month_state_invalidate(&self, month: Month) -> Result<()> {
        // 여기서 `DELETE FROM month_state`를 쓰면 관측 1건 추가마다 전체 타임라인이
        // 재계산된다. 달력 월 버킷을 고른 이유 자체가 사라진다 (§12.5).
        self.conn.execute(
            "DELETE FROM month_state WHERE month = ?1",
            rusqlite::params![month.to_string()],
        )?;
        Ok(())
    }

    /// 캐시된 군집과 그때의 관측 수.
    pub fn cluster_get(&self) -> Result<Option<(usize, Clustering)>> {
        let row: Option<(i64, i64, Vec<u8>, Vec<u8>)> = self
            .conn
            .query_row(
                "SELECT n, k, centroids, assignment FROM cluster_cache WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((n, k, centroids, assignment)) = row else {
            return Ok(None);
        };
        let k = usize::try_from(k)
            .map_err(|_| SoulError::invalid(format!("군집 캐시의 k가 음수입니다: {k}")))?;
        if k == 0 {
            return Err(SoulError::invalid("군집 캐시의 k가 0입니다"));
        }
        let flat =
            from_f16_blob(&centroids).ok_or_else(|| SoulError::invalid("손상된 군집 중심 블롭"))?;
        if flat.len() % k != 0 {
            return Err(SoulError::invalid(format!(
                "군집 중심 블롭이 k={k}로 나누어떨어지지 않습니다 ({}개 원소)",
                flat.len()
            )));
        }
        let dims = flat.len() / k;
        let centroids: Vec<Vec<f32>> = flat.chunks_exact(dims).map(|c| c.to_vec()).collect();
        let assignment = decode_assignment(&assignment)?;
        Ok(Some((
            n.max(0) as usize,
            Clustering {
                centroids,
                assignment,
                k,
            },
        )))
    }

    pub fn cluster_put(&self, n: usize, c: &Clustering) -> Result<()> {
        // 중심을 하나의 f16 블롭으로 눕혀 저장하므로 길이가 균일해야 복원된다.
        if c.centroids.len() != c.k {
            return Err(SoulError::invalid(format!(
                "군집 중심 개수({})가 k({})와 다릅니다",
                c.centroids.len(),
                c.k
            )));
        }
        if c.k == 0 {
            return Err(SoulError::invalid("k = 0인 군집은 저장하지 않습니다"));
        }
        let dims = c.centroids[0].len();
        if c.centroids.iter().any(|v| v.len() != dims) {
            return Err(SoulError::invalid("군집 중심의 차원이 섞여 있습니다"));
        }
        let mut flat: Vec<f32> = Vec::with_capacity(dims * c.k);
        for v in &c.centroids {
            flat.extend_from_slice(v);
        }
        // 캐시는 한 행만 쓴다 (id = 1). 이전 캐시는 덮어쓴다.
        self.conn.execute(
            "INSERT INTO cluster_cache(id, n, k, centroids, assignment) VALUES(1, ?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
                 n = excluded.n, k = excluded.k,
                 centroids = excluded.centroids, assignment = excluded.assignment",
            rusqlite::params![
                n as i64,
                c.k as i64,
                to_f16_blob(&flat),
                encode_assignment(&c.assignment)?
            ],
        )?;
        Ok(())
    }

    /// 활성 ingest들의 동결된 `min_dist` 목록 (§12.4의 `D`).
    pub fn min_dists_get(&self) -> Result<Vec<f64>> {
        // `obs_vec`이 아니라 `ingest_frozen`에서 읽는다. §R4대로 투입 시점 값이며
        // 군집이 바뀌어도 다시 계산하지 않는다. `null`인 것은 `D`에 넣지 않는다 (§12.4).
        let mut stmt = self.conn.prepare(
            "SELECT min_dist FROM ingest_frozen WHERE min_dist IS NOT NULL ORDER BY obs_id ASC",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, f64>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// §12.4의 동결값을 캐시에 적어 둔다. 원본은 관측 JSON이며 여기는 사영이다 (§R4).
    /// supersede된 ingest는 넣지 않는다 (§R9).
    pub fn ingest_frozen_put(
        &self,
        obs_id: &str,
        min_dist: Option<f64>,
        surprisal: f64,
        ts: Ts,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO ingest_frozen(obs_id, min_dist, surprisal, ts) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(obs_id) DO UPDATE SET
                 min_dist = excluded.min_dist,
                 surprisal = excluded.surprisal,
                 ts = excluded.ts",
            rusqlite::params![obs_id, min_dist, surprisal, ts.to_rfc3339_millis()],
        )?;
        Ok(())
    }

    /// supersede된 ingest를 `D`에서 뺄 때 쓴다 (§R9, T17).
    pub fn ingest_frozen_remove(&self, obs_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM ingest_frozen WHERE obs_id = ?1",
            rusqlite::params![obs_id],
        )?;
        Ok(())
    }

    pub fn pca_get(&self, t_ref_date: &str) -> Result<Option<Vec<(f64, f64)>>> {
        let blob: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT coords FROM pca_cache WHERE t_ref_date = ?1",
                rusqlite::params![t_ref_date],
                |r| r.get(0),
            )
            .optional()?;
        match blob {
            None => Ok(None),
            Some(b) => decode_coords(&b).map(Some),
        }
    }

    pub fn pca_put(&self, t_ref_date: &str, coords: &[(f64, f64)]) -> Result<()> {
        // `T_ref`의 날짜가 바뀌면 새 행이 생긴다. 지난 날짜 행은 그대로 두어도
        // 무해하지만, 대시보드가 매번 한 행만 읽으므로 여기서 정리한다 (§13 화면 6).
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        tx.execute(
            "DELETE FROM pca_cache WHERE t_ref_date <> ?1",
            rusqlite::params![t_ref_date],
        )?;
        tx.execute(
            "INSERT INTO pca_cache(t_ref_date, coords) VALUES(?1, ?2)
             ON CONFLICT(t_ref_date) DO UPDATE SET coords = excluded.coords",
            rusqlite::params![t_ref_date, encode_coords(coords)],
        )?;
        tx.commit()?;
        Ok(())
    }
}

/// 군집 배정은 u32 LE로 눕힌다. k ≤ 8이므로 값은 늘 작다.
fn encode_assignment(a: &[usize]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(a.len() * 4);
    for &x in a {
        let x = u32::try_from(x)
            .map_err(|_| SoulError::invalid(format!("군집 배정 값이 u32를 넘습니다: {x}")))?;
        out.extend_from_slice(&x.to_le_bytes());
    }
    Ok(out)
}

fn decode_assignment(b: &[u8]) -> Result<Vec<usize>> {
    if !b.len().is_multiple_of(4) {
        return Err(SoulError::invalid(format!(
            "손상된 군집 배정 블롭: {}바이트는 4의 배수가 아닙니다",
            b.len()
        )));
    }
    Ok(b.chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]) as usize)
        .collect())
}

/// PCA 좌표는 f64 LE 쌍이다. 화면 좌표라 §20.3의 f16 대상이 아니며,
/// 2회 실행 시 좌표가 **비트 동일**해야 한다 (T69).
fn encode_coords(coords: &[(f64, f64)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(coords.len() * 16);
    for (x, y) in coords {
        out.extend_from_slice(&x.to_le_bytes());
        out.extend_from_slice(&y.to_le_bytes());
    }
    out
}

fn decode_coords(b: &[u8]) -> Result<Vec<(f64, f64)>> {
    if !b.len().is_multiple_of(16) {
        return Err(SoulError::invalid(format!(
            "손상된 PCA 좌표 블롭: {}바이트는 16의 배수가 아닙니다",
            b.len()
        )));
    }
    Ok(b.chunks_exact(16)
        .map(|c| {
            let x = f64::from_le_bytes(c[0..8].try_into().expect("8바이트"));
            let y = f64::from_le_bytes(c[8..16].try_into().expect("8바이트"));
            (x, y)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    fn ms(month: &str, drift: Option<f64>, crystal: Option<f64>, n: usize) -> MonthState {
        MonthState {
            month: Month::parse(month).unwrap(),
            drift,
            crystal,
            n,
        }
    }

    #[test]
    fn month_state_roundtrips_with_nulls() {
        let db = Db::open_in_memory().unwrap();
        let s = ms("2026-08", None, Some(0.42), 17);
        db.month_state_put(&s).unwrap();
        assert_eq!(db.month_state_get(s.month).unwrap(), Some(s));
        assert!(db.month_state_get(Month::new(2026, 7)).unwrap().is_none());
    }

    #[test]
    fn month_state_put_overwrites_the_same_month() {
        let db = Db::open_in_memory().unwrap();
        db.month_state_put(&ms("2026-08", Some(0.1), Some(0.2), 3))
            .unwrap();
        db.month_state_put(&ms("2026-08", Some(0.9), None, 4))
            .unwrap();
        let got = db.month_state_get(Month::new(2026, 8)).unwrap().unwrap();
        assert_eq!(got.drift, Some(0.9));
        assert_eq!(got.crystal, None);
        assert_eq!(got.n, 4);
    }

    #[test]
    fn invalidate_touches_only_that_month() {
        // T40 — 관측 1건 추가 → 지난 달 캐시가 재계산되지 않는다.
        let db = Db::open_in_memory().unwrap();
        for m in ["2026-06", "2026-07", "2026-08"] {
            db.month_state_put(&ms(m, Some(0.3), Some(0.5), 10))
                .unwrap();
        }
        db.month_state_invalidate(Month::new(2026, 8)).unwrap();

        assert!(db.month_state_get(Month::new(2026, 8)).unwrap().is_none());
        assert!(db.month_state_get(Month::new(2026, 7)).unwrap().is_some());
        assert!(db.month_state_get(Month::new(2026, 6)).unwrap().is_some());
    }

    #[test]
    fn invalidating_an_absent_month_is_a_no_op() {
        let db = Db::open_in_memory().unwrap();
        db.month_state_put(&ms("2026-07", None, None, 1)).unwrap();
        db.month_state_invalidate(Month::new(2026, 8)).unwrap();
        assert!(db.month_state_get(Month::new(2026, 7)).unwrap().is_some());
    }

    #[test]
    fn cluster_roundtrips() {
        let db = Db::open_in_memory().unwrap();
        assert!(db.cluster_get().unwrap().is_none());

        let c = Clustering {
            centroids: vec![vec![1.0, 0.0, 0.0], vec![0.0, 0.5, -0.5]],
            assignment: vec![0, 1, 1, 0, 1],
            k: 2,
        };
        db.cluster_put(40, &c).unwrap();
        let (n, back) = db.cluster_get().unwrap().unwrap();
        assert_eq!(n, 40);
        assert_eq!(back.k, 2);
        assert_eq!(back.assignment, c.assignment);
        assert_eq!(back.centroids.len(), 2);
        assert_eq!(back.centroids[0].len(), 3, "차원이 보존되어야 한다");
        for (a, b) in c
            .centroids
            .iter()
            .flatten()
            .zip(back.centroids.iter().flatten())
        {
            assert!((a - b).abs() < 1e-3, "f16 손실 범위 내여야 한다");
        }
    }

    #[test]
    fn cluster_put_keeps_a_single_row() {
        let db = Db::open_in_memory().unwrap();
        let c = Clustering {
            centroids: vec![vec![1.0, 0.0], vec![0.0, 1.0]],
            assignment: vec![0, 1],
            k: 2,
        };
        db.cluster_put(4, &c).unwrap();
        db.cluster_put(9, &c).unwrap();
        let rows: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM cluster_cache", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1);
        assert_eq!(db.cluster_get().unwrap().unwrap().0, 9);
    }

    #[test]
    fn cluster_put_rejects_inconsistent_input() {
        let db = Db::open_in_memory().unwrap();
        let bad_k = Clustering {
            centroids: vec![vec![1.0, 0.0]],
            assignment: vec![0],
            k: 2,
        };
        assert!(matches!(
            db.cluster_put(4, &bad_k).unwrap_err(),
            SoulError::Invalid(_)
        ));
        let ragged = Clustering {
            centroids: vec![vec![1.0, 0.0], vec![0.0]],
            assignment: vec![0, 1],
            k: 2,
        };
        assert!(matches!(
            db.cluster_put(4, &ragged).unwrap_err(),
            SoulError::Invalid(_)
        ));
    }

    #[test]
    fn min_dists_skips_nulls_and_is_ordered() {
        // §12.4의 `D` — null이 아닌 값들만. `|D|`가 분모가 되므로 개수가 곧 의미다.
        let db = Db::open_in_memory().unwrap();
        let ts = Ts::parse("2026-08-13T09:12:33.123Z").unwrap();
        db.ingest_frozen_put("01C", Some(0.7), 0.9, ts).unwrap();
        db.ingest_frozen_put("01A", None, 1.0, ts).unwrap();
        db.ingest_frozen_put("01B", Some(0.3), 0.2, ts).unwrap();

        assert_eq!(db.min_dists_get().unwrap(), vec![0.3, 0.7]);
    }

    #[test]
    fn min_dists_is_empty_before_any_ingest() {
        // T18 — |D| = 0에서도 조회가 크래시하지 않는다.
        let db = Db::open_in_memory().unwrap();
        assert!(db.min_dists_get().unwrap().is_empty());
    }

    #[test]
    fn frozen_values_can_be_removed_when_superseded() {
        // §R9 — supersede된 ingest는 §12의 모든 계산에서 빠진다 (T17).
        let db = Db::open_in_memory().unwrap();
        let ts = Ts::parse("2026-08-13T09:12:33.123Z").unwrap();
        db.ingest_frozen_put("01A", Some(0.3), 0.2, ts).unwrap();
        db.ingest_frozen_put("01B", Some(0.5), 0.4, ts).unwrap();
        db.ingest_frozen_remove("01A").unwrap();
        assert_eq!(db.min_dists_get().unwrap(), vec![0.5]);
    }

    #[test]
    fn pca_roundtrips_bit_exactly() {
        // T69 — PCA 투영 2회 실행 시 좌표 동일. 캐시가 값을 흔들면 안 된다.
        let db = Db::open_in_memory().unwrap();
        let coords = vec![(0.123_456_789_012_345, -1.5), (2.0, 0.0)];
        db.pca_put("2026-08-13", &coords).unwrap();
        assert_eq!(db.pca_get("2026-08-13").unwrap().unwrap(), coords);
        assert!(db.pca_get("2026-08-12").unwrap().is_none());
    }

    #[test]
    fn pca_put_drops_other_reference_dates() {
        // §13 화면 6 — `T_ref`의 날짜가 바뀌면 이전 좌표는 쓸모가 없다.
        let db = Db::open_in_memory().unwrap();
        db.pca_put("2026-08-12", &[(1.0, 1.0)]).unwrap();
        db.pca_put("2026-08-13", &[(2.0, 2.0)]).unwrap();
        assert!(db.pca_get("2026-08-12").unwrap().is_none());
        assert_eq!(db.pca_get("2026-08-13").unwrap().unwrap(), [(2.0, 2.0)]);
    }

    #[test]
    fn empty_pca_coords_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        db.pca_put("2026-08-13", &[]).unwrap();
        assert_eq!(db.pca_get("2026-08-13").unwrap().unwrap(), Vec::new());
    }

    #[test]
    fn corrupt_blobs_error_instead_of_panicking() {
        let db = Db::open_in_memory().unwrap();
        db.conn
            .execute(
                "INSERT INTO pca_cache(t_ref_date, coords) VALUES('2026-08-13', x'0011')",
                [],
            )
            .unwrap();
        assert!(matches!(
            db.pca_get("2026-08-13").unwrap_err(),
            SoulError::Invalid(_)
        ));

        db.conn
            .execute(
                "INSERT INTO cluster_cache(id, n, k, centroids, assignment) \
                 VALUES(1, 4, 3, x'00000000', x'00')",
                [],
            )
            .unwrap();
        assert!(matches!(
            db.cluster_get().unwrap_err(),
            SoulError::Invalid(_)
        ));
    }
}
