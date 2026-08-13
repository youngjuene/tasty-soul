//! sqlite 스키마와 연결 (§12 · §20.3).
//!
//! ## 테이블
//!
//! | 테이블 | 내용 | §12.7 공간 |
//! |---|---|---|
//! | `embed_cache` | `key TEXT PK, dims INT, vec BLOB` — f16 고정 길이 | — |
//! | `obs_vec` | `obs_id TEXT PK, vec BLOB` — `ingest.machine.prose` 임베딩 | **주 공간** |
//! | `critique_vec` | `obs_id TEXT PK, vec BLOB` — `context.critique` · cultural `reading.prose` | **별도 공간** |
//! | `month_state` | `month TEXT PK, drift REAL, crystal REAL, n INT` | — |
//! | `cluster_cache` | `id INT PK, n INT, k INT, centroids BLOB, assignment BLOB` | — |
//! | `pca_cache` | `t_ref_date TEXT PK, coords BLOB` | — |
//! | `critique_queue` | §9.10 영속 큐 | — |
//! | `meta` | `key TEXT PK, value TEXT` — 스키마 버전 등 | — |
//!
//! **`critique_vec`을 `obs_vec`과 합치지 말 것** (§12.7, §18-3, T49).
//! 비평문은 대상 자체가 아니라 그 주변에 관한 글이다. 같은 공간에 섞으면 군집이
//! "무엇을 좋아하는가"에서 "무엇에 대해 글이 쓰였는가"로 흐르고, 결과는 여전히 그럴듯해 보인다.

use crate::error::{Result, SoulError};
use rusqlite::OptionalExtension;
use std::path::Path;

pub const SCHEMA_VERSION: i64 = 1;

/// sqlite 수준의 짧은 경합은 기다린다. §R7의 쓰기 락(즉시 에러)과는 다른 층이다 —
/// 비평 워커(§9.10)와 앱 스레드가 같은 파일을 동시에 만지는 것은 정상 상황이다.
const BUSY_TIMEOUT_MS: u64 = 5_000;

/// 전체 스키마. 전부 `IF NOT EXISTS`이므로 `migrate()`가 몇 번 불려도 안전하다.
///
/// `ingest_frozen`은 §12.4의 `D`다. `min_dist`·`surprisal`은 투입 시점에 동결되며(§R4)
/// 관측 JSON이 원본이다. 이 테이블은 그 사영(projection)이므로 재빌드로 복구된다.
const DDL: &str = "
CREATE TABLE IF NOT EXISTS meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS embed_cache (
  key  TEXT PRIMARY KEY,
  dims INTEGER NOT NULL,
  vec  BLOB NOT NULL
);

CREATE TABLE IF NOT EXISTS obs_vec (
  obs_id TEXT PRIMARY KEY,
  vec    BLOB NOT NULL
);

CREATE TABLE IF NOT EXISTS critique_vec (
  obs_id TEXT PRIMARY KEY,
  vec    BLOB NOT NULL
);

CREATE TABLE IF NOT EXISTS ingest_frozen (
  obs_id    TEXT PRIMARY KEY,
  min_dist  REAL,
  surprisal REAL NOT NULL,
  ts        TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS month_state (
  month   TEXT PRIMARY KEY,
  drift   REAL,
  crystal REAL,
  n       INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS cluster_cache (
  id         INTEGER PRIMARY KEY,
  n          INTEGER NOT NULL,
  k          INTEGER NOT NULL,
  centroids  BLOB NOT NULL,
  assignment BLOB NOT NULL
);

CREATE TABLE IF NOT EXISTS pca_cache (
  t_ref_date TEXT PRIMARY KEY,
  coords     BLOB NOT NULL
);

CREATE TABLE IF NOT EXISTS critique_queue (
  ingest_id  TEXT PRIMARY KEY,
  state      TEXT NOT NULL,
  attempts   INTEGER NOT NULL DEFAULT 0,
  last_error TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS critique_queue_by_state ON critique_queue(state);
";

impl std::fmt::Debug for Db {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Db")
    }
}

pub struct Db {
    pub conn: rusqlite::Connection,
}

impl Db {
    /// 파일을 열고 필요하면 스키마를 만든다. WAL 모드를 켠다.
    pub fn open(path: &Path) -> Result<Db> {
        // `cache/`는 삭제 가능한 디렉토리다(§3). 사용자가 지웠으면 다시 만든다.
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let db = Db {
            conn: rusqlite::Connection::open(path)?,
        };
        db.configure(true)?;
        db.migrate()?;
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Db> {
        let db = Db {
            conn: rusqlite::Connection::open_in_memory()?,
        };
        // 메모리 DB에는 WAL이 없다(`journal_mode`가 `memory`로 고정된다).
        db.configure(false)?;
        db.migrate()?;
        Ok(db)
    }

    /// 읽기 전용으로 연다. `soul-mcp`가 쓴다 (§19.6, T35).
    /// 파일이 없으면 **명확한 에러**를 낸다. 크래시하지 않는다 (T36).
    pub fn open_read_only(path: &Path) -> Result<Db> {
        // sqlite에 먼저 물으면 "unable to open database file"이라는 모호한 문구가 나온다.
        // 무엇이 없는지 경로째로 알려준다 (T36).
        if !path.is_file() {
            return Err(SoulError::MissingPath(path.to_path_buf()));
        }
        let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
            | rusqlite::OpenFlags::SQLITE_OPEN_URI
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let db = Db {
            conn: rusqlite::Connection::open_with_flags(read_only_uri(path)?, flags)?,
        };
        db.conn
            .busy_timeout(std::time::Duration::from_millis(BUSY_TIMEOUT_MS))?;
        db.conn.execute_batch("PRAGMA foreign_keys=ON;")?;

        // 읽기 전용이라 `migrate()`를 부를 수 없다. 빈 파일·다른 파일을 물고
        // 첫 질의에서 터지는 대신 여기서 사유를 밝힌다 (T36).
        let present: Option<String> = db
            .conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='embed_cache'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if present.is_none() {
            return Err(SoulError::invalid(format!(
                "{} 에 tasty-soul 스키마가 없습니다. 앱을 한 번 실행해 캐시를 만드십시오",
                path.display()
            )));
        }
        Ok(db)
    }

    pub fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(DDL)?;
        match self.schema_version()? {
            Some(v) if v == SCHEMA_VERSION => {}
            // 미래 버전의 캐시를 구버전 코드가 열면 조용히 깨지는 대신 거부한다.
            // `derived.sqlite`는 삭제 가능하므로(§3) 사용자는 지우고 재빌드하면 된다.
            Some(v) if v > SCHEMA_VERSION => {
                return Err(SoulError::invalid(format!(
                    "derived.sqlite 스키마 버전 {v} 은 이 빌드({SCHEMA_VERSION})보다 새롭습니다. \
                     cache/derived.sqlite 를 지우고 다시 여십시오"
                )));
            }
            // 없거나(신규) 낮은 버전(구버전 캐시) — 지금은 v1뿐이라 표기만 갱신한다.
            _ => {
                self.conn.execute(
                    "INSERT INTO meta(key, value) VALUES('schema_version', ?1)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    rusqlite::params![SCHEMA_VERSION.to_string()],
                )?;
            }
        }
        Ok(())
    }

    /// `meta`에 적힌 스키마 버전. 아직 마이그레이션 전이면 `None`.
    pub fn schema_version(&self) -> Result<Option<i64>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key='schema_version'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        Ok(raw.and_then(|s| s.trim().parse::<i64>().ok()))
    }

    /// 파생 테이블만 비운다. **`embed_cache`는 보존한다** (T2).
    pub fn clear_derived(&self) -> Result<()> {
        // 보존: `embed_cache`(재임베딩 = 네트워크, §R3) ·
        //       `obs_vec`/`critique_vec`(관측 ID ↔ 벡터 연결. 이게 남아야 T2가
        //        오프라인으로 성립한다) · `critique_queue`(파생값이 아니라 미완료 작업.
        //        지우면 대기 중인 투입이 영영 비평되지 않는다, §9.10) · `meta`.
        // 비움:  관측 로그만으로 재계산되는 것 전부.
        self.conn.execute_batch(
            "BEGIN IMMEDIATE;
             DELETE FROM month_state;
             DELETE FROM cluster_cache;
             DELETE FROM pca_cache;
             DELETE FROM ingest_frozen;
             COMMIT;",
        )?;
        Ok(())
    }

    /// 연결 단위 pragma. `wal = false`는 메모리 DB용이다.
    fn configure(&self, wal: bool) -> Result<()> {
        self.conn
            .busy_timeout(std::time::Duration::from_millis(BUSY_TIMEOUT_MS))?;
        self.conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        if wal {
            // `journal_mode`는 결과 행을 돌려주므로 `execute`가 아니라 `query_row`다.
            let _mode: String = self
                .conn
                .query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        }
        Ok(())
    }
}

/// `file:<경로>?mode=ro`. URI 문법에서 뜻이 있는 문자만 퍼센트 인코딩한다.
fn read_only_uri(path: &Path) -> Result<String> {
    let s = path
        .to_str()
        .ok_or_else(|| SoulError::MissingPath(path.to_path_buf()))?;
    let mut out = String::with_capacity(s.len() + 16);
    for ch in s.chars() {
        match ch {
            '%' => out.push_str("%25"),
            '?' => out.push_str("%3f"),
            '#' => out.push_str("%23"),
            // 윈도우 구분자. sqlite URI는 `/`만 받는다.
            '\\' => out.push('/'),
            c => out.push(c),
        }
    }
    Ok(format!("file:{out}?mode=ro"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::embed_cache::Space;

    fn table_exists(db: &Db, name: &str) -> bool {
        db.conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type='table' AND name=?1",
                rusqlite::params![name],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .unwrap()
            .is_some()
    }

    fn count(db: &Db, table: &str) -> i64 {
        db.conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn migrate_creates_every_table_and_stamps_version() {
        let db = Db::open_in_memory().unwrap();
        for t in [
            "meta",
            "embed_cache",
            "obs_vec",
            "critique_vec",
            "ingest_frozen",
            "month_state",
            "cluster_cache",
            "pca_cache",
            "critique_queue",
        ] {
            assert!(table_exists(&db, t), "{t} 테이블이 없다");
        }
        assert_eq!(db.schema_version().unwrap(), Some(SCHEMA_VERSION));
    }

    #[test]
    fn migrate_is_idempotent() {
        let db = Db::open_in_memory().unwrap();
        db.migrate().unwrap();
        db.migrate().unwrap();
        assert_eq!(db.schema_version().unwrap(), Some(SCHEMA_VERSION));
        assert_eq!(count(&db, "meta"), 1);
    }

    #[test]
    fn open_enables_wal_and_foreign_keys() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("nested").join("derived.sqlite")).unwrap();
        let mode: String = db
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
        let fk: i64 = db
            .conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1, "PRAGMA foreign_keys=ON");
    }

    #[test]
    fn open_creates_missing_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache").join("derived.sqlite");
        let db = Db::open(&path).unwrap();
        db.embed_put("k", 2, &[0.25, 0.5]).unwrap();
        drop(db);
        assert!(path.is_file());
        // 다시 열어도 내용이 남아 있다.
        let db = Db::open(&path).unwrap();
        assert_eq!(db.embed_count().unwrap(), 1);
    }

    #[test]
    fn clear_derived_keeps_embed_cache_and_vectors() {
        // T2 — derived.sqlite를 통째로 지우지 않는 경로. 임베딩은 네트워크 자원이므로
        // 파생값만 날리고 벡터는 남긴다.
        let db = Db::open_in_memory().unwrap();
        db.embed_put("key-1", 4, &[0.1, 0.2, 0.3, 0.4]).unwrap();
        db.obs_vec_put(Space::Object, "01OBS", &[0.1, 0.2, 0.3, 0.4])
            .unwrap();
        db.obs_vec_put(Space::Critique, "01OBS", &[0.4, 0.3, 0.2, 0.1])
            .unwrap();
        db.queue_push("01OBS").unwrap();
        db.conn
            .execute(
                "INSERT INTO month_state(month, drift, crystal, n) VALUES('2026-08', 0.1, 0.2, 3)",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO pca_cache(t_ref_date, coords) VALUES('2026-08-13', x'00')",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO cluster_cache(id, n, k, centroids, assignment) \
                 VALUES(1, 4, 2, x'00', x'00')",
                [],
            )
            .unwrap();
        db.ingest_frozen_put("01OBS", Some(0.3), 0.5, crate::time::Ts::now())
            .unwrap();

        db.clear_derived().unwrap();

        assert_eq!(db.embed_count().unwrap(), 1, "embed_cache 보존 (T2)");
        assert!(db.obs_vec_get(Space::Object, "01OBS").unwrap().is_some());
        assert!(db.obs_vec_get(Space::Critique, "01OBS").unwrap().is_some());
        assert_eq!(db.queue_pending_count().unwrap(), 1, "미완료 작업 보존");
        assert_eq!(db.schema_version().unwrap(), Some(SCHEMA_VERSION));

        assert_eq!(count(&db, "month_state"), 0);
        assert_eq!(count(&db, "pca_cache"), 0);
        assert_eq!(count(&db, "cluster_cache"), 0);
        assert_eq!(count(&db, "ingest_frozen"), 0);
    }

    #[test]
    fn open_read_only_missing_file_errors_without_panic() {
        // T36 — derived.sqlite 없는 상태에서 명확한 에러. 크래시 없음.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("derived.sqlite");
        // `Db`는 Debug가 아니므로 unwrap_err()를 쓸 수 없다. match로 받는다.
        match Db::open_read_only(&path) {
            Err(SoulError::MissingPath(p)) => assert_eq!(p, path),
            Err(other) => panic!("MissingPath 를 기대했다: {other}"),
            Ok(_) => panic!("없는 파일이 열렸다"),
        }
    }

    #[test]
    fn open_read_only_rejects_a_file_without_our_schema() {
        // 빈 파일이나 남의 sqlite를 물었을 때도 크래시하지 않는다 (T36).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("derived.sqlite");
        std::fs::write(&path, b"").unwrap();
        match Db::open_read_only(&path) {
            Err(SoulError::Invalid(_)) => {}
            Err(other) => panic!("Invalid 를 기대했다: {other}"),
            Ok(_) => panic!("스키마 없는 파일이 통과했다"),
        }
    }

    #[test]
    fn open_read_only_reads_but_cannot_write() {
        // T35 — 읽기 전용으로 열어도 조회는 정상 동작한다 (§19.6).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("derived.sqlite");
        {
            let db = Db::open(&path).unwrap();
            db.obs_vec_put(Space::Object, "01OBS", &[1.0, 0.0]).unwrap();
        }
        let ro = Db::open_read_only(&path).unwrap();
        assert_eq!(ro.obs_vec_all(Space::Object).unwrap().len(), 1);
        assert!(
            ro.obs_vec_put(Space::Object, "01OTHER", &[0.0, 1.0])
                .is_err(),
            "읽기 전용 연결에 쓰기가 통과하면 안 된다 (§19.6)"
        );
    }

    #[test]
    fn read_only_uri_escapes_uri_syntax() {
        let uri = read_only_uri(Path::new("/tmp/a b?c#d%e/derived.sqlite")).unwrap();
        assert_eq!(uri, "file:/tmp/a b%3fc%23d%25e/derived.sqlite?mode=ro");
    }
}
