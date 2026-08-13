//! 에이전트 트레이스 (§11.3).
//!
//! 모든 API 호출을 `runs/<ts>.jsonl`에 한 줄씩 기록한다.
//! **재현성의 일부가 아니며 언제든 삭제 가능하다.**
//!
//! `response_raw`에는 서술문과 프롬프트가 평문으로 남는다. 로컬 파일이지만
//! **사용자가 지울 수단을 반드시 제공한다** — `soul trace purge` (§14, T71).

use crate::error::Result;
use crate::paths::Paths;
use crate::time::Ts;
use serde::{Deserialize, Serialize};
use std::io::Write;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TraceEntry {
    pub ts: Ts,
    /// `describe` | `merge` | `critique` | `reflect` | `embed` | `doctor` | ...
    pub purpose: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_sha256: Option<String>,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_usd_est: f64,
    pub latency_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_raw: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 한 세션의 트레이스 파일. 세션 시작 시각으로 이름을 정한다.
pub struct Tracer {
    path: std::path::PathBuf,
}

impl Tracer {
    /// `runs/2026-08-13T09-12-33Z.jsonl` 를 연다.
    ///
    /// 파일을 미리 만들어 둔다 — `path()`가 가리키는 경로는 항상 존재한다.
    pub fn open(paths: &Paths) -> Result<Tracer> {
        let dir = paths.runs();
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(file_name_for(Ts::now()));
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        Ok(Tracer { path })
    }

    pub fn write(&self, entry: &TraceEntry) -> Result<()> {
        // 한 줄 = 한 JSON. `serde_json`이 문자열 안의 개행을 이스케이프하므로
        // `response_raw`가 여러 줄이어도 jsonl 한 줄이 유지된다.
        let mut line = serde_json::to_string(entry)?;
        line.push('\n');
        // 호출마다 append로 다시 연다. 트레이스는 재현성의 일부가 아니므로
        // 처리량보다 크래시 시 유실이 적은 쪽을 택한다 (§11.3).
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        f.write_all(line.as_bytes())?;
        Ok(())
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

/// 트레이스 파일 이름. **콜론을 쓰지 않는다** — Windows 파일명에 못 들어간다.
/// `2026-08-13T09:12:33.123Z` → `2026-08-13T09-12-33Z.jsonl` (초 단위).
fn file_name_for(ts: Ts) -> String {
    format!("{}.jsonl", ts.as_datetime().format("%Y-%m-%dT%H-%M-%SZ"))
}

/// `soul trace purge` — `runs/` 전체 삭제. 관측과 `SOUL.md`는 건드리지 않는다 (T71).
///
/// 반환값은 지운 항목 수. 디렉토리 자체는 남긴다 — 앱이 곧바로 다음 트레이스를 연다.
pub fn purge(paths: &Paths) -> Result<usize> {
    let dir = paths.runs();
    if !dir.is_dir() {
        return Ok(0);
    }
    let mut removed = 0usize;
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(&path)?;
        } else {
            std::fs::remove_file(&path)?;
        }
        removed += 1;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_paths() -> Paths {
        let root = std::env::temp_dir()
            .join("tasty-soul-trace-test")
            .join(crate::ids::new_id().to_string());
        let p = Paths::at(root);
        p.ensure_dirs().unwrap();
        p
    }

    fn sample(purpose: &str, response: Option<&str>) -> TraceEntry {
        TraceEntry {
            ts: Ts::parse("2026-08-13T09:12:33.123Z").unwrap(),
            purpose: purpose.to_string(),
            model: "gpt-x".into(),
            prompt_sha256: Some("9f2c1a".repeat(11)[..64].to_string()),
            tokens_in: 1200,
            tokens_out: 340,
            cost_usd_est: 0.0123,
            latency_ms: 812,
            response_raw: response.map(|s| s.to_string()),
            error: None,
        }
    }

    #[test]
    fn file_name_has_no_colon() {
        let ts = Ts::parse("2026-08-13T09:12:33.123Z").unwrap();
        assert_eq!(file_name_for(ts), "2026-08-13T09-12-33Z.jsonl");
        assert!(!file_name_for(ts).contains(':'), "콜론은 파일명에 못 쓴다");
    }

    #[test]
    fn writes_one_json_object_per_line() {
        let paths = temp_paths();
        let t = Tracer::open(&paths).unwrap();
        assert!(t.path().is_file(), "open은 파일을 만들어 둔다");
        t.write(&sample("describe", Some("첫 줄\n둘째 줄")))
            .unwrap();
        t.write(&sample("embed", None)).unwrap();

        let body = std::fs::read_to_string(t.path()).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2, "한 호출 = 한 줄이어야 한다: {body:?}");
        let first: TraceEntry = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first.purpose, "describe");
        assert_eq!(first.response_raw.as_deref(), Some("첫 줄\n둘째 줄"));
        let second: TraceEntry = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(second.purpose, "embed");
        assert!(second.response_raw.is_none());
        assert!(body.ends_with('\n'));
    }

    #[test]
    fn write_appends_across_tracers_on_same_path() {
        let paths = temp_paths();
        let a = Tracer::open(&paths).unwrap();
        a.write(&sample("doctor", None)).unwrap();
        // 같은 세션 파일을 다시 열어도 덮어쓰지 않는다.
        let b = Tracer {
            path: a.path().to_path_buf(),
        };
        b.write(&sample("doctor", None)).unwrap();
        assert_eq!(
            std::fs::read_to_string(a.path()).unwrap().lines().count(),
            2
        );
    }

    /// T71 — purge 후 `runs/`만 비고 관측·`SOUL.md`는 불변이다.
    #[test]
    fn purge_clears_runs_only() {
        let paths = temp_paths();

        let soul_md = paths.soul_md();
        std::fs::write(&soul_md, "# SOUL\n").unwrap();
        let ts = Ts::parse("2026-08-13T09:12:33.123Z").unwrap();
        let id = crate::ids::new_id();
        let obs_file = paths.observation_file(ts, &id);
        std::fs::create_dir_all(obs_file.parent().unwrap()).unwrap();
        std::fs::write(&obs_file, "{\n  \"type\": \"ingest\"\n}\n").unwrap();

        let t = Tracer::open(&paths).unwrap();
        t.write(&sample("describe", Some("평문 서술"))).unwrap();
        std::fs::write(paths.runs().join("2026-08-12T00-00-00Z.jsonl"), "{}\n").unwrap();

        let removed = purge(&paths).unwrap();
        assert_eq!(removed, 2, "runs/의 두 파일을 지워야 한다");

        assert!(paths.runs().is_dir(), "runs/ 디렉토리 자체는 남긴다");
        assert_eq!(std::fs::read_dir(paths.runs()).unwrap().count(), 0);

        assert_eq!(std::fs::read_to_string(&soul_md).unwrap(), "# SOUL\n");
        assert_eq!(
            std::fs::read_to_string(&obs_file).unwrap(),
            "{\n  \"type\": \"ingest\"\n}\n"
        );
        assert!(paths.observations().is_dir());
    }

    #[test]
    fn purge_without_runs_dir_is_zero() {
        let root = std::env::temp_dir()
            .join("tasty-soul-trace-test")
            .join(crate::ids::new_id().to_string());
        let paths = Paths::at(root);
        assert_eq!(purge(&paths).unwrap(), 0);
    }
}
