//! 비평 작업 큐 (§9.10).
//!
//! ```text
//! ingest 기록 직후 → cache/derived.sqlite의 critique_queue에 (ingest_id, 상태) 삽입
//!                 → 앱이 실행 중이면 워커가 집어간다
//!                 → 완료 시 context 관측 기록 + 알림 → 화면 2.1
//! ```
//!
//! - 큐는 **`derived.sqlite`에 영속화한다.** 앱이 중간에 닫히면 다음 실행 때 이어받는다 (T51c).
//! - 동시 실행은 `critique_concurrency`개로 제한한다 (T51d).
//! - **상주 프로세스를 만들지 않는다** (§20.1). 워커는 앱 프로세스 안에서만 돌고,
//!   앱을 닫으면 함께 멈춘다 (T44).
//! - API 실패는 3회까지 재시도 후 큐에서 제거하고 트레이스에 남긴다.

use crate::error::{Result, SoulError};
use crate::time::Ts;
use rusqlite::{OptionalExtension, Row, Transaction, TransactionBehavior};

/// §9.10 — 여기까지 실패하면 더 시도하지 않는다.
pub const MAX_ATTEMPTS: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueState {
    Pending,
    Running,
    Failed,
    Done,
}

impl QueueState {
    pub fn as_str(&self) -> &'static str {
        match self {
            QueueState::Pending => "pending",
            QueueState::Running => "running",
            QueueState::Failed => "failed",
            QueueState::Done => "done",
        }
    }

    pub fn parse(s: &str) -> Option<QueueState> {
        match s {
            "pending" => Some(QueueState::Pending),
            "running" => Some(QueueState::Running),
            "failed" => Some(QueueState::Failed),
            "done" => Some(QueueState::Done),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct QueueItem {
    pub ingest_id: String,
    pub state: QueueState,
    pub attempts: u32,
    pub last_error: Option<String>,
}

impl super::Db {
    /// `ingest` 기록 직후에 부른다 (T51b).
    pub fn queue_push(&self, ingest_id: &str) -> Result<()> {
        // 재등록은 `soul context --redo`(§14)의 경로이기도 하다. 다만 워커가 이미
        // 집어간 항목은 되돌리지 않는다 — 그러면 같은 건이 두 번 처리된다.
        let now = Ts::now().to_rfc3339_millis();
        self.conn.execute(
            "INSERT INTO critique_queue(ingest_id, state, attempts, last_error, created_at, updated_at)
             VALUES(?1, 'pending', 0, NULL, ?2, ?2)
             ON CONFLICT(ingest_id) DO UPDATE SET
                 state      = 'pending',
                 attempts   = 0,
                 last_error = NULL,
                 updated_at = excluded.updated_at
             WHERE critique_queue.state <> 'running'",
            rusqlite::params![ingest_id, now],
        )?;
        Ok(())
    }

    /// 앱 시작 시 `running` 상태를 `pending`으로 되돌린다 (T51c —
    /// 이전 실행이 중간에 죽었을 수 있다).
    pub fn queue_recover(&self) -> Result<usize> {
        // `attempts`는 건드리지 않는다. §9.10의 3회는 **API 실패** 횟수이지
        // 앱을 닫은 횟수가 아니다.
        let n = self.conn.execute(
            "UPDATE critique_queue SET state = 'pending', updated_at = ?1 WHERE state = 'running'",
            rusqlite::params![Ts::now().to_rfc3339_millis()],
        )?;
        Ok(n)
    }

    /// `pending` 하나를 `running`으로 바꾸고 반환한다. 없으면 `None`.
    pub fn queue_claim(&self) -> Result<Option<QueueItem>> {
        // BEGIN IMMEDIATE로 시작한다. DEFERRED면 워커 둘이 같은 행을 읽은 뒤
        // 쓰기 승격에서 하나가 튕기고, 최악의 경우 같은 건을 두 번 처리한다 (T51d).
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        // ULID 오름차순 = 투입 순서. 먼저 들어온 것을 먼저 집는다.
        let picked: Option<QueueItem> = tx
            .query_row(
                "SELECT ingest_id, state, attempts, last_error FROM critique_queue
                 WHERE state = 'pending' ORDER BY ingest_id ASC LIMIT 1",
                [],
                row_to_item,
            )
            .optional()?
            .transpose()?;
        let Some(item) = picked else {
            tx.commit()?;
            return Ok(None);
        };
        tx.execute(
            "UPDATE critique_queue SET state = 'running', updated_at = ?2 WHERE ingest_id = ?1",
            rusqlite::params![item.ingest_id, Ts::now().to_rfc3339_millis()],
        )?;
        tx.commit()?;
        Ok(Some(QueueItem {
            state: QueueState::Running,
            ..item
        }))
    }

    pub fn queue_complete(&self, ingest_id: &str) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE critique_queue SET state = 'done', last_error = NULL, updated_at = ?2
             WHERE ingest_id = ?1",
            rusqlite::params![ingest_id, Ts::now().to_rfc3339_millis()],
        )?;
        if n == 0 {
            return Err(SoulError::invalid(format!(
                "비평 큐에 없는 항목입니다: {ingest_id}"
            )));
        }
        Ok(())
    }

    /// 실패를 기록한다. `attempts >= 3`이면 `failed`로 굳힌다.
    pub fn queue_fail(&self, ingest_id: &str, error: &str) -> Result<QueueState> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let prior: Option<i64> = tx
            .query_row(
                "SELECT attempts FROM critique_queue WHERE ingest_id = ?1",
                rusqlite::params![ingest_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(prior) = prior else {
            return Err(SoulError::invalid(format!(
                "비평 큐에 없는 항목입니다: {ingest_id}"
            )));
        };
        let attempts = prior.max(0) as u32 + 1;
        // `failed`는 큐에서 사라지는 대신 굳는 상태다. 화면 6이 "문화 글귀 없음"과
        // 재시도 버튼을 띄우려면 흔적이 남아 있어야 한다 (§9.10).
        let state = if attempts >= MAX_ATTEMPTS {
            QueueState::Failed
        } else {
            QueueState::Pending
        };
        tx.execute(
            "UPDATE critique_queue SET state = ?2, attempts = ?3, last_error = ?4, updated_at = ?5
             WHERE ingest_id = ?1",
            rusqlite::params![
                ingest_id,
                state.as_str(),
                attempts as i64,
                error,
                Ts::now().to_rfc3339_millis()
            ],
        )?;
        tx.commit()?;
        Ok(state)
    }

    /// 화면 1에 조용히 표시할 대기 건수 (§9.10 · §13 화면 2.1).
    pub fn queue_pending_count(&self) -> Result<usize> {
        // `running`은 세지 않는다. 실행 중인 건수는 `queue_items(Some(Running))`이다.
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM critique_queue WHERE state = 'pending'",
            [],
            |r| r.get(0),
        )?;
        Ok(n.max(0) as usize)
    }

    pub fn queue_items(&self, state: Option<QueueState>) -> Result<Vec<QueueItem>> {
        let sql = "SELECT ingest_id, state, attempts, last_error FROM critique_queue";
        let mut out = Vec::new();
        match state {
            Some(s) => {
                let mut stmt = self
                    .conn
                    .prepare(&format!("{sql} WHERE state = ?1 ORDER BY ingest_id ASC"))?;
                let rows = stmt.query_map(rusqlite::params![s.as_str()], row_to_item)?;
                for r in rows {
                    out.push(r??);
                }
            }
            None => {
                let mut stmt = self
                    .conn
                    .prepare(&format!("{sql} ORDER BY ingest_id ASC"))?;
                let rows = stmt.query_map([], row_to_item)?;
                for r in rows {
                    out.push(r??);
                }
            }
        }
        Ok(out)
    }
}

/// 행 → `QueueItem`. 상태 문자열이 깨져 있으면 `SoulError`로 감싼다.
#[allow(clippy::type_complexity)]
fn row_to_item(r: &Row<'_>) -> rusqlite::Result<Result<QueueItem>> {
    let ingest_id: String = r.get(0)?;
    let raw: String = r.get(1)?;
    let attempts: i64 = r.get(2)?;
    let last_error: Option<String> = r.get(3)?;
    Ok(match QueueState::parse(&raw) {
        Some(state) => Ok(QueueItem {
            ingest_id,
            state,
            attempts: attempts.max(0) as u32,
            last_error,
        }),
        None => Err(SoulError::invalid(format!(
            "알 수 없는 큐 상태 '{raw}' ({ingest_id})"
        ))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    fn state_of(db: &Db, id: &str) -> QueueState {
        db.queue_items(None)
            .unwrap()
            .into_iter()
            .find(|i| i.ingest_id == id)
            .expect("큐에 있어야 한다")
            .state
    }

    #[test]
    fn push_registers_exactly_one_row() {
        // T51b — 투입 1건 → critique_queue에 항목 1건.
        let db = Db::open_in_memory().unwrap();
        db.queue_push("01A").unwrap();
        db.queue_push("01A").unwrap();
        assert_eq!(db.queue_items(None).unwrap().len(), 1);
        assert_eq!(db.queue_pending_count().unwrap(), 1);
    }

    #[test]
    fn claim_moves_pending_to_running_and_is_exclusive() {
        let db = Db::open_in_memory().unwrap();
        db.queue_push("01B").unwrap();
        db.queue_push("01A").unwrap();

        let first = db.queue_claim().unwrap().unwrap();
        assert_eq!(first.ingest_id, "01A", "ULID 오름차순 = 투입 순서");
        assert_eq!(first.state, QueueState::Running);
        assert_eq!(db.queue_pending_count().unwrap(), 1);

        let second = db.queue_claim().unwrap().unwrap();
        assert_eq!(second.ingest_id, "01B");
        assert!(
            db.queue_claim().unwrap().is_none(),
            "이미 집어간 항목을 다시 집으면 안 된다"
        );
        assert_eq!(db.queue_pending_count().unwrap(), 0);
    }

    #[test]
    fn claim_on_empty_queue_is_none() {
        let db = Db::open_in_memory().unwrap();
        assert!(db.queue_claim().unwrap().is_none());
    }

    #[test]
    fn recover_returns_running_items_to_pending() {
        // T51c — 큐 처리 중 앱 종료 → 재실행 시 미완료 항목을 이어받는다.
        let db = Db::open_in_memory().unwrap();
        db.queue_push("01A").unwrap();
        db.queue_push("01B").unwrap();
        db.queue_claim().unwrap().unwrap();
        db.queue_complete("01B").unwrap();
        // 여기서 앱이 죽었다고 치자. 01A는 running으로 남아 있다.

        let recovered = db.queue_recover().unwrap();
        assert_eq!(recovered, 1, "running 이던 1건만 되살린다");
        assert_eq!(state_of(&db, "01A"), QueueState::Pending);
        assert_eq!(state_of(&db, "01B"), QueueState::Done, "완료분은 그대로");
        assert_eq!(db.queue_claim().unwrap().unwrap().ingest_id, "01A");
    }

    #[test]
    fn recover_keeps_attempts() {
        // 앱을 닫은 것은 API 실패가 아니다. 재시도 예산을 깎지 않는다.
        let db = Db::open_in_memory().unwrap();
        db.queue_push("01A").unwrap();
        db.queue_claim().unwrap();
        db.queue_fail("01A", "429").unwrap();
        db.queue_claim().unwrap();
        db.queue_recover().unwrap();
        assert_eq!(state_of(&db, "01A"), QueueState::Pending);
        assert_eq!(db.queue_items(None).unwrap()[0].attempts, 1);
    }

    #[test]
    fn fail_retries_twice_then_freezes_as_failed() {
        // §9.10 — API 실패는 3회까지 재시도 후 큐에서 제거하고 트레이스에 남긴다.
        let db = Db::open_in_memory().unwrap();
        db.queue_push("01A").unwrap();

        assert_eq!(
            db.queue_fail("01A", "timeout").unwrap(),
            QueueState::Pending
        );
        assert_eq!(db.queue_fail("01A", "500").unwrap(), QueueState::Pending);
        assert_eq!(db.queue_fail("01A", "500").unwrap(), QueueState::Failed);

        let item = &db.queue_items(Some(QueueState::Failed)).unwrap()[0];
        assert_eq!(item.attempts, MAX_ATTEMPTS);
        assert_eq!(item.last_error.as_deref(), Some("500"));
        assert_eq!(db.queue_pending_count().unwrap(), 0);
        assert!(
            db.queue_claim().unwrap().is_none(),
            "굳은 실패는 더 집지 않는다"
        );
    }

    #[test]
    fn complete_marks_done_and_clears_error() {
        let db = Db::open_in_memory().unwrap();
        db.queue_push("01A").unwrap();
        db.queue_fail("01A", "timeout").unwrap();
        db.queue_complete("01A").unwrap();

        let item = &db.queue_items(None).unwrap()[0];
        assert_eq!(item.state, QueueState::Done);
        assert!(item.last_error.is_none());
        assert_eq!(db.queue_pending_count().unwrap(), 0);
    }

    #[test]
    fn unknown_id_errors_instead_of_silently_passing() {
        let db = Db::open_in_memory().unwrap();
        assert!(matches!(
            db.queue_complete("없음").unwrap_err(),
            SoulError::Invalid(_)
        ));
        assert!(matches!(
            db.queue_fail("없음", "e").unwrap_err(),
            SoulError::Invalid(_)
        ));
    }

    #[test]
    fn push_requeues_a_failed_item_but_never_a_running_one() {
        // `soul context --redo` (§14) 경로. 실행 중인 건은 되돌리지 않는다.
        let db = Db::open_in_memory().unwrap();
        db.queue_push("01A").unwrap();
        for _ in 0..MAX_ATTEMPTS {
            db.queue_fail("01A", "500").unwrap();
        }
        assert_eq!(state_of(&db, "01A"), QueueState::Failed);
        db.queue_push("01A").unwrap();
        assert_eq!(state_of(&db, "01A"), QueueState::Pending);
        assert_eq!(db.queue_items(None).unwrap()[0].attempts, 0);

        db.queue_claim().unwrap().unwrap();
        db.queue_push("01A").unwrap();
        assert_eq!(
            state_of(&db, "01A"),
            QueueState::Running,
            "워커가 집어간 항목을 pending으로 되돌리면 같은 건이 두 번 처리된다"
        );
    }

    #[test]
    fn queue_survives_reopen() {
        // 큐는 derived.sqlite에 영속화된다 (§9.10, T51c).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("derived.sqlite");
        {
            let db = Db::open(&path).unwrap();
            db.queue_push("01A").unwrap();
            db.queue_claim().unwrap().unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(state_of(&db, "01A"), QueueState::Running);
        assert_eq!(db.queue_recover().unwrap(), 1);
        assert_eq!(db.queue_pending_count().unwrap(), 1);
    }

    #[test]
    fn items_filter_by_state() {
        let db = Db::open_in_memory().unwrap();
        db.queue_push("01A").unwrap();
        db.queue_push("01B").unwrap();
        db.queue_claim().unwrap();
        assert_eq!(db.queue_items(Some(QueueState::Running)).unwrap().len(), 1);
        assert_eq!(db.queue_items(Some(QueueState::Pending)).unwrap().len(), 1);
        assert_eq!(db.queue_items(Some(QueueState::Done)).unwrap().len(), 0);
        assert_eq!(db.queue_items(None).unwrap().len(), 2);
    }

    #[test]
    fn corrupt_state_string_errors_instead_of_panicking() {
        let db = Db::open_in_memory().unwrap();
        db.conn
            .execute(
                "INSERT INTO critique_queue(ingest_id, state, attempts, created_at, updated_at)
                 VALUES('01A', '???', 0, '2026-08-13T00:00:00.000Z', '2026-08-13T00:00:00.000Z')",
                [],
            )
            .unwrap();
        assert!(matches!(
            db.queue_items(None).unwrap_err(),
            SoulError::Invalid(_)
        ));
    }
}
