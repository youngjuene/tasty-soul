//! 비평 작업 워커 (§9.10).
//!
//! - **모든 `ingest`에 대해 자동으로 실행한다.** 사용자 요청을 기다리지 않는다
//! - 동시 실행은 `critique_concurrency`개로 제한 (T51d)
//! - **상주 프로세스를 만들지 않는다** (§20.1) — 앱 프로세스 안에서만 돈다
//! - API 실패는 3회까지 재시도 후 큐에서 제거하고 트레이스에 남긴다
//! - 검색 근거를 하나도 못 찾으면 `context` 관측을 만들지 않는다 (T52)
//! - **알림은 건별로 띄우지 않는다** (§13 화면 2.1, T51e). 대기 건수만 갱신한다

use soul_agent::critique::CritiqueInput;
use soul_core::db::queue::{QueueItem, QueueState, MAX_ATTEMPTS};
use soul_core::db::Db;
use soul_core::error::{Result, SoulError};
use soul_core::ids::ObsId;
use soul_core::obs::{ContextObs, Observation, Source};
use soul_core::time::Ts;
use soul_core::trace::{TraceEntry, Tracer};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

/// 취소 신호 되묻는 간격. `AtomicBool`에는 대기 수단이 없으므로 짧게 폴링한다.
/// 이 값이 곧 "앱을 닫고 워커가 멈추기까지"의 상한이다 (§20.1, T44).
const CANCEL_POLL_MS: u64 = 20;

/// T52 — 근거를 못 찾은 항목이 큐에 남기는 사유.
const NO_GROUND: &str = "검색 근거를 찾지 못해 context 관측을 만들지 않았습니다 (§11.1)";

pub struct Worker {
    /// 취소 신호. 앱 종료 시 내린다.
    pub cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Worker {
    /// 큐가 빌 때까지 처리한다. 앱 시작 시 한 번 호출하고, 새 투입마다 깨운다.
    ///
    /// 반환값은 **새로 기록된 `context` 관측 수**다. 실패한 건은 세지 않는다 —
    /// 화면 2.1이 이 수만큼 대기 건수를 올린다 (T51e: 건별 알림이 아니다).
    pub async fn drain(&self, app: &crate::App) -> Result<usize> {
        let th = &app.config.thresholds;

        // T54 — 문화 층이 꺼져 있으면 큐를 아예 건드리지 않는다. 검색 요청 0건.
        if !th.context_enabled {
            return Ok(0);
        }
        // §20.1 · T44 — 이미 닫히는 중이면 아무것도 시작하지 않는다.
        if self.cancel.load(Ordering::Relaxed) {
            return Ok(0);
        }
        // 집을 것이 없으면 클라이언트를 만들지 않는다 — 키체인과 네트워크를 건드릴 이유가 없다.
        //
        // `running`을 되살리는 것은 앱 시작의 `queue_recover` 몫이다 (§9.10, T51c).
        // 워커가 그것까지 하면 동시에 도는 다른 `drain`이 처리 중인 항목을 빼앗는다.
        if app.db.queue_pending_count()? == 0 {
            return Ok(0);
        }

        let shared = Shared::build(app)?;
        pump(
            &app.db,
            &self.cancel,
            th.critique_concurrency,
            |item| {
                // 준비 실패(관측 없음·깨진 ID)도 정상적인 실패다. 태스크 안으로 들고
                // 들어가서 다른 실패와 같은 자리에서 큐에 기록한다.
                let prepared = critique_input(app, &item.ingest_id).map_err(|e| e.to_string());
                let shared = shared.clone();
                async move {
                    match prepared {
                        Ok(input) => run_job(input, shared).await,
                        Err(e) => JobOutcome::Failed(e),
                    }
                }
            },
            |item, outcome| settle(app, item, outcome),
        )
        .await
    }
}

/// `soul context <ingest_id> [--redo]` (§14 · §9.10 실패 재시도).
///
/// 큐를 거치지 않고 즉시 1건 처리한다. 반환값은 만들어진 `context` 관측 ID이며,
/// **근거를 못 찾았으면 `None`이다** (T52 — 실패는 정상적인 결과다).
pub async fn run_one(app: &crate::App, ingest_id: &soul_core::ids::ObsId) -> Result<Option<ObsId>> {
    // §9.10 비활성화 — "비평을 아예 생성하지 않는다". 명령이라고 해서 뚫지 않는다.
    if !app.config.thresholds.context_enabled {
        return Err(SoulError::config(
            "context_enabled = false 입니다. 문화 글귀를 만들지 않습니다 (§9.10)",
        ));
    }
    // 워커가 이미 집어간 건을 겹쳐 처리하면 같은 대상에 context가 두 개 생긴다.
    if let Some(item) = queue_item(&app.db, ingest_id.as_str())? {
        if item.state == QueueState::Running {
            return Err(SoulError::invalid(format!(
                "{ingest_id} 은(는) 비평 워커가 처리 중입니다. 끝난 뒤에 다시 시도하세요"
            )));
        }
    }

    let input = critique_input(app, ingest_id.as_str())?;
    let shared = Shared::build(app)?;
    match run_job(input, shared).await {
        JobOutcome::Context(ctx) => {
            let id = write_context(app, *ctx)?;
            mark_done(app, ingest_id.as_str())?;
            Ok(Some(id))
        }
        // T52 — 관측을 만들지 않는다. 에러도 아니다. 화면 6이 "문화 글귀 없음"으로 보인다.
        JobOutcome::NoGround => {
            record_failure(app, ingest_id.as_str(), NO_GROUND, true)?;
            Ok(None)
        }
        // 명시적 명령이므로 호출 실패는 그대로 알린다 (§20.4).
        JobOutcome::Failed(msg) => {
            record_failure(app, ingest_id.as_str(), &msg, false)?;
            Err(SoulError::invalid(msg))
        }
    }
}

// ─────────────────────────────────────────────────────────── 한 건의 작업

/// 한 건의 비평 결과. **실패는 정상적인 결과다** (§9.10).
enum JobOutcome {
    /// 근거를 찾았다. 관측으로 기록한다.
    Context(Box<ContextObs>),
    /// T52 — 근거를 하나도 못 찾았다. **관측을 만들지 않는다.**
    NoGround,
    /// 호출 실패. `queue_fail`이 attempts를 센다.
    Failed(String),
}

/// 모든 작업이 공유하는 입력. `drain` 한 번에 한 벌만 만든다.
///
/// 전부 소유값이다 — 작업은 `JoinSet`에 `'static`으로 들어가므로 `&App`을 들고 갈 수 없다.
#[derive(Clone)]
struct Shared {
    client: Arc<soul_net::OpenAi>,
    model: String,
    search: soul_core::config::SearchConfig,
    search_key: Option<String>,
    thresholds: soul_core::config::Thresholds,
    tracer: Option<Arc<Tracer>>,
}

impl Shared {
    fn build(app: &crate::App) -> Result<Shared> {
        // §10.4의 비평은 텍스트 호출이다. `reflect` 슬롯은 성찰 에이전트 전용이므로
        // 여기서는 `text`를 쓴다 (§9.8 — 모델 ID를 코드에 하드코딩하지 않는다).
        let model = app.config.models.text.clone();
        if model.trim().is_empty() {
            return Err(SoulError::config(
                "models.text 가 비어 있습니다. `soul doctor`로 모델을 고르세요 (§9.9)",
            ));
        }
        // `Tracer`는 복제할 수 없으므로 이 drain 몫으로 하나 연다. 같은 `runs/` 파일에
        // append 되므로 앱의 트레이서와 내용이 섞이지 않는다 (§11.3).
        let tracer = match &app.tracer {
            Some(_) => Some(Arc::new(Tracer::open(&app.paths)?)),
            None => None,
        };
        Ok(Shared {
            client: Arc::new(app.openai()?),
            model,
            search: app.config.search.clone(),
            search_key: search_key(&app.config),
            thresholds: app.config.thresholds.clone(),
            tracer,
        })
    }
}

/// 검색 제공자 키 (`docs/OPEN-DECISIONS.md` #14).
///
/// `config.toml`의 값이 있으면 그것을 쓰고, 없을 때만 키체인을 본다 —
/// 설정에 적어 둔 사용자가 키체인 접근 프롬프트를 다시 보지 않게 한다 (§2).
fn search_key(cfg: &soul_core::config::Config) -> Option<String> {
    if !cfg.search.api_key.trim().is_empty() {
        return Some(cfg.search.api_key.clone());
    }
    soul_net::secrets::get(soul_net::secrets::ACCOUNT_SEARCH)
        .ok()
        .flatten()
        .filter(|k| !k.trim().is_empty())
}

/// §11.1 툴콜 루프 한 건. **`&App`을 건드리지 않는다** — `JoinSet`에서 돌기 때문이다.
async fn run_job(input: CritiqueInput, shared: Shared) -> JobOutcome {
    let out = soul_agent::critique::run(
        &input,
        &shared.client,
        &shared.model,
        &shared.search,
        shared.search_key.as_deref(),
        &shared.thresholds,
        shared.tracer.as_deref(),
    )
    .await;
    match out {
        Ok(o) => match o.context {
            Some(c) => JobOutcome::Context(Box::new(c)),
            None => JobOutcome::NoGround,
        },
        Err(e) => JobOutcome::Failed(e.to_string()),
    }
}

/// 작업 결과를 관측과 큐에 반영한다. **관측을 쓰는 쪽은 여기 하나뿐이다.**
///
/// 반환값이 `true`면 `context` 관측이 하나 늘었다는 뜻이다.
fn settle(app: &crate::App, item: &QueueItem, outcome: JobOutcome) -> Result<bool> {
    match outcome {
        JobOutcome::Context(ctx) => match write_context(app, *ctx) {
            Ok(_) => {
                app.db.queue_complete(&item.ingest_id)?;
                Ok(true)
            }
            // 관측을 못 쓴 것도 실패다 — 다음 시도에서 다시 만든다.
            Err(e) => {
                record_failure(app, &item.ingest_id, &e.to_string(), false)?;
                Ok(false)
            }
        },
        // T52 — 관측 없이 실패로만 남긴다. **에러 팝업을 만들지 않는다.**
        JobOutcome::NoGround => {
            record_failure(app, &item.ingest_id, NO_GROUND, true)?;
            Ok(false)
        }
        JobOutcome::Failed(msg) => {
            record_failure(app, &item.ingest_id, &msg, false)?;
            Ok(false)
        }
    }
}

/// `context` 관측 기록 + 커밋 (§R7 · §R8).
///
/// **재렌더를 유발하지 않는다** (§R8 — 재렌더 계기는 넷뿐이며 `context`는 그중에 없다).
fn write_context(app: &crate::App, ctx: ContextObs) -> Result<ObsId> {
    let soul_dir = app.paths.soul();
    // §R7 — 쓰기는 락 안에서. 실패하면 대기하지 않고 즉시 에러다 (§15).
    let _lock = soul_core::lock::WriteLock::acquire(&soul_dir)?;
    soul_core::git::ensure_repo(&soul_dir)?;

    let obs = Observation::Context(ctx);
    let written = app.store.append(&obs)?;
    // §R8 — 쓰기 1회 = 커밋 1개. 메시지는 `<type> <ULID>`.
    soul_core::git::commit_paths(
        &soul_dir,
        &[&written],
        &format!("{} {}", obs.type_name(), obs.id()),
    )?;
    Ok(obs.id().clone())
}

/// 실패를 큐와 트레이스에 남긴다 (§9.10).
///
/// `terminal`이면 재시도 예산을 즉시 소진해 `failed`로 굳힌다. 근거 없음(T52)이 그렇다 —
/// 같은 입력으로 다시 물어도 결과가 같으므로 즉시 재시도하면 검색만 세 배로 나간다.
/// 나중에 다시 시도할 값어치는 있으므로 §13 화면 6의 재시도 버튼(`soul context --redo`)이
/// 남는다. API 실패는 `terminal = false`이며 3회까지 자연스럽게 재시도된다.
fn record_failure(
    app: &crate::App,
    ingest_id: &str,
    reason: &str,
    terminal: bool,
) -> Result<QueueState> {
    ensure_queued(&app.db, ingest_id)?;
    let mut state = app.db.queue_fail(ingest_id, reason)?;
    if terminal {
        // `queue_fail`은 한 번에 1회분만 센다. 굳을 때까지 부른다 (상한이 있으므로 유한하다).
        for _ in 0..MAX_ATTEMPTS {
            if state == QueueState::Failed {
                break;
            }
            state = app.db.queue_fail(ingest_id, reason)?;
        }
    }
    // §9.10 — 트레이스에 남긴다. 트레이스 실패가 큐 처리를 막지 않는다 (§11.3).
    if let Some(tracer) = &app.tracer {
        let _ = tracer.write(&TraceEntry {
            ts: Ts::now(),
            purpose: "critique".to_string(),
            model: app.config.models.text.clone(),
            prompt_sha256: None,
            tokens_in: 0,
            tokens_out: 0,
            cost_usd_est: 0.0,
            latency_ms: 0,
            response_raw: None,
            error: Some(format!("{ingest_id}: {reason}")),
        });
    }
    Ok(state)
}

/// 큐에 항목이 없으면 넣는다. `run_one`이 큐를 거치지 않고 처리한 건도
/// 화면 6이 "문화 글귀 없음 + 재시도"를 그리려면 흔적이 남아 있어야 한다 (§9.10).
fn ensure_queued(db: &Db, ingest_id: &str) -> Result<()> {
    if queue_item(db, ingest_id)?.is_none() {
        db.queue_push(ingest_id)?;
    }
    Ok(())
}

fn mark_done(app: &crate::App, ingest_id: &str) -> Result<()> {
    ensure_queued(&app.db, ingest_id)?;
    app.db.queue_complete(ingest_id)
}

fn queue_item(db: &Db, ingest_id: &str) -> Result<Option<QueueItem>> {
    Ok(db
        .queue_items(None)?
        .into_iter()
        .find(|i| i.ingest_id == ingest_id))
}

/// §10.4 — `kind`별 추가 입력을 관측에서 뽑는다.
fn critique_input(app: &crate::App, ingest_id: &str) -> Result<CritiqueInput> {
    let id = ObsId::parse(ingest_id)?;
    let obs = app.store.read(&id)?;
    let ing = obs
        .as_ingest()
        .ok_or_else(|| SoulError::invalid(format!("{ingest_id} 은(는) ingest 관측이 아닙니다")))?;
    Ok(CritiqueInput {
        target_id: ing.id.clone(),
        kind: ing.source.kind,
        machine_prose: ing.machine.prose.clone(),
        tags: ing.machine.tags.clone(),
        extra: extra_from_source(&ing.source),
    })
}

/// §10.4의 "추가 입력". **관측 로그가 가진 것만 넘긴다.**
///
/// 원본 파일도 붙여넣은 텍스트도 앱 데이터로 복사하지 않으므로(T42) 사후에 되살릴 수 있는
/// 실마리는 `source.origin`뿐이다. YouTube는 URL을 그대로 넘겨 에이전트가 `web_fetch`로
/// 제목·채널을 읽게 하고, 로컬 파일은 파일명만, 붙여넣은 텍스트는 실마리가 없으므로 `None`이다.
fn extra_from_source(src: &Source) -> Option<String> {
    if let Some(path) = src.origin.strip_prefix("file://") {
        return std::path::Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.to_string())
            .filter(|n| !n.is_empty());
    }
    if src.origin.starts_with("http://") || src.origin.starts_with("https://") {
        return Some(src.origin.clone());
    }
    // `clipboard:<sha12>` — 원문이 남아 있지 않다.
    None
}

// ───────────────────────────────────────────────────────────── 동시성 펌프

/// 큐가 빌 때까지 집어 돌린다. **동시 실행은 `concurrency`개를 넘지 않는다** (T51d).
///
/// 실제 작업은 `start`가 만드는 future가 하고, 결과 반영은 `settle`이 한다.
/// 이 분리 덕분에 (a) sqlite 연결을 태스크로 보내지 않아도 되고 —`Db`는 `Sync`가 아니다—
/// (b) 관측을 쓰는 지점이 한 군데로 모이며 (c) 가짜 작업으로 상한과 취소를 시험할 수 있다.
async fn pump<T, F, Fut, S>(
    db: &Db,
    cancel: &AtomicBool,
    concurrency: usize,
    mut start: F,
    mut settle: S,
) -> Result<usize>
where
    T: Send + 'static,
    F: FnMut(QueueItem) -> Fut,
    Fut: std::future::Future<Output = T> + Send + 'static,
    S: FnMut(&QueueItem, T) -> Result<bool>,
{
    // 0이면 아무것도 못 한다. 설정 실수를 조용한 정지로 만들지 않는다.
    let limit = concurrency.max(1);
    let sem = Arc::new(Semaphore::new(limit));
    let mut set: JoinSet<(QueueItem, T)> = JoinSet::new();
    let mut written = 0usize;

    loop {
        // ── 매 루프마다 취소를 확인한다 (§20.1, T44).
        if cancel.load(Ordering::Relaxed) {
            set.abort_all();
            break;
        }

        // ── 이미 끝난 작업을 먼저 거둔다. 큐 항목을 오래 `running`으로 두지 않는다.
        while let Some(joined) = set.try_join_next() {
            if let Some((item, out)) = joined_or_lost(joined) {
                if settle(&item, out)? {
                    written += 1;
                }
            }
        }

        // ── 자리가 있으면 하나 집는다. **자리 수가 곧 동시 실행 상한이다.**
        //    permit은 태스크 본문이 끝나는 순간 반납되므로, 아직 거두지 않은 완료분은
        //    상한을 잡아먹지 않는다.
        if let Ok(permit) = Arc::clone(&sem).try_acquire_owned() {
            match db.queue_claim()? {
                Some(item) => {
                    let fut = start(item.clone());
                    set.spawn(async move {
                        let _permit = permit;
                        (item, fut.await)
                    });
                    continue;
                }
                None => {
                    drop(permit);
                    // 큐도 비고 진행 중인 것도 없으면 끝이다.
                    if set.is_empty() {
                        break;
                    }
                }
            }
        }

        // ── 큐가 비었거나 자리가 없다. 하나가 끝나기를 기다리되 취소를 함께 감시한다.
        //    이 select가 없으면 90초짜리 호출이 끝날 때까지 앱이 닫히지 못한다.
        tokio::select! {
            biased;
            _ = cancelled(cancel) => {
                set.abort_all();
                break;
            }
            joined = set.join_next() => match joined {
                Some(j) => {
                    if let Some((item, out)) = joined_or_lost(j) {
                        if settle(&item, out)? {
                            written += 1;
                        }
                    }
                }
                None => break,
            }
        }
    }

    Ok(written)
}

/// 태스크가 패닉했거나 abort되면 결과가 없다.
///
/// 그 항목은 큐에 `running`으로 남고 다음 실행의 `queue_recover`가 되살린다 (T51c).
/// 여기서 즉시 되돌리면 패닉하는 항목이 같은 drain 안에서 무한히 다시 집힌다.
fn joined_or_lost<T>(
    joined: std::result::Result<(QueueItem, T), tokio::task::JoinError>,
) -> Option<(QueueItem, T)> {
    joined.ok()
}

/// `cancel`이 설 때까지 기다린다. `AtomicBool`에는 대기 수단이 없어 짧게 되묻는다.
async fn cancelled(cancel: &AtomicBool) {
    while !cancel.load(Ordering::Relaxed) {
        tokio::time::sleep(std::time::Duration::from_millis(CANCEL_POLL_MS)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::testkit::temp_app;
    use soul_core::obs::{ModelRef, SourceRef};
    use soul_core::SCHEMA_VERSION;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    // ─────────────────────────────────────────────────────────── 픽스처

    fn id(n: u32) -> ObsId {
        ObsId::parse(&format!("01J8XQZK3M7P4RSTVWXYZ{n:05}")).expect("ULID")
    }

    fn model_ref() -> ModelRef {
        ModelRef {
            provider: "openai".into(),
            id: "gpt-x".into(),
            prompt_sha256: None,
            calls: vec![],
        }
    }

    fn context_obs(n: u32, target: ObsId) -> ContextObs {
        let fetched_at = Ts::parse("2026-08-13T09:12:33.123Z").expect("ts");
        let sources = vec![
            SourceRef {
                url: "https://example.org/a".into(),
                title: "가".into(),
                fetched_at,
            },
            SourceRef {
                url: "https://example.org/b".into(),
                title: "나".into(),
                fetched_at,
            },
        ];
        ContextObs {
            id: id(n),
            ts: fetched_at,
            schema: SCHEMA_VERSION,
            target,
            critique: "이 사진은 1970년대 뉴토포그래픽스의 어법을 따른다.".into(),
            lineage: vec!["뉴토포그래픽스".into()],
            queries: vec!["뉴토포그래픽스 실내".into()],
            // §6.4 — 파이프라인이 센다. 2건이므로 true.
            grounded: sources.len() >= 2,
            sources,
            model: model_ref(),
        }
    }

    fn queue_state(db: &Db, ingest_id: &str) -> QueueState {
        queue_item(db, ingest_id)
            .unwrap()
            .expect("큐에 있어야 한다")
            .state
    }

    // ───────────────────────────────────────────── T51d — 동시 실행 상한

    /// 가짜 작업으로 상한을 시험한다. 실제 에이전트를 부르지 않는다.
    #[tokio::test]
    async fn concurrency_never_exceeds_the_limit() {
        let db = Db::open_in_memory().unwrap();
        for n in 0..8u32 {
            db.queue_push(&format!("01{n:022}")).unwrap();
        }
        let cancel = AtomicBool::new(false);
        let live = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let done = pump(
            &db,
            &cancel,
            3,
            |_item| {
                let live = Arc::clone(&live);
                let peak = Arc::clone(&peak);
                async move {
                    let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    live.fetch_sub(1, Ordering::SeqCst);
                }
            },
            |item, ()| {
                db.queue_complete(&item.ingest_id)?;
                Ok(true)
            },
        )
        .await
        .unwrap();

        assert_eq!(done, 8, "큐가 빌 때까지 처리한다");
        assert_eq!(db.queue_pending_count().unwrap(), 0);
        let peak = peak.load(Ordering::SeqCst);
        assert!(peak <= 3, "동시 실행이 상한을 넘었다: {peak}");
        assert!(peak > 1, "상한 안에서는 실제로 동시에 돌아야 한다: {peak}");
    }

    /// 상한 1이면 한 번에 하나만 돈다.
    #[tokio::test]
    async fn concurrency_of_one_is_sequential() {
        let db = Db::open_in_memory().unwrap();
        for n in 0..4u32 {
            db.queue_push(&format!("01{n:022}")).unwrap();
        }
        let cancel = AtomicBool::new(false);
        let live = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        pump(
            &db,
            &cancel,
            1,
            |_item| {
                let live = Arc::clone(&live);
                let peak = Arc::clone(&peak);
                async move {
                    let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    live.fetch_sub(1, Ordering::SeqCst);
                }
            },
            |item, ()| {
                db.queue_complete(&item.ingest_id)?;
                Ok(true)
            },
        )
        .await
        .unwrap();

        assert_eq!(peak.load(Ordering::SeqCst), 1);
    }

    // ───────────────────────────────────────────── T44 — 취소 신호

    /// 진행 중인 작업이 길어도 취소 신호가 서면 루프가 곧바로 끝난다 (§20.1).
    #[tokio::test]
    async fn cancel_stops_the_loop_without_waiting_for_the_job() {
        let db = Db::open_in_memory().unwrap();
        for n in 0..4u32 {
            db.queue_push(&format!("01{n:022}")).unwrap();
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            flag.store(true, Ordering::SeqCst);
        });

        let started = std::time::Instant::now();
        let done = pump(
            &db,
            &cancel,
            1,
            // 앱을 닫아도 끝나지 않을 만큼 긴 작업.
            |_item| async { tokio::time::sleep(Duration::from_secs(30)).await },
            |item, ()| {
                db.queue_complete(&item.ingest_id)?;
                Ok(true)
            },
        )
        .await
        .unwrap();

        assert!(
            started.elapsed() < Duration::from_secs(5),
            "취소 후에도 작업을 기다렸다: {:?}",
            started.elapsed()
        );
        assert_eq!(done, 0);
        assert!(
            db.queue_items(Some(QueueState::Done)).unwrap().is_empty(),
            "취소된 작업을 완료로 표시하면 안 된다"
        );
        // 남은 항목은 pending/running 그대로다. 다음 실행이 `queue_recover`로 이어받는다 (T51c).
        assert_eq!(db.queue_items(None).unwrap().len(), 4);
    }

    /// 이미 취소된 상태로 들어오면 큐를 집지도 않는다.
    #[tokio::test]
    async fn pre_set_cancel_claims_nothing() {
        let db = Db::open_in_memory().unwrap();
        db.queue_push("01A").unwrap();
        let cancel = AtomicBool::new(true);

        let done = pump(
            &db,
            &cancel,
            2,
            |_item| async { panic!("취소 상태에서는 작업을 시작하면 안 된다") },
            |_item, ()| Ok(true),
        )
        .await
        .unwrap();

        assert_eq!(done, 0);
        assert_eq!(queue_state(&db, "01A"), QueueState::Pending, "집지 않았다");
    }

    // ───────────────────────────────────────────── drain 의 짧은 경로

    /// T54 — `context_enabled = false`면 큐를 건드리지 않는다. 검색 요청 0건.
    ///
    /// (키가 없어 `openai()`가 실패하므로, 이 테스트가 통과한다는 것 자체가
    ///  클라이언트를 만들지 않았다는 증거다.)
    #[tokio::test]
    async fn drain_does_nothing_when_context_is_disabled() {
        let mut fx = temp_app("worker-disabled");
        fx.app.config.thresholds.context_enabled = false;
        let app = &fx.app;
        app.db.queue_push("01A").unwrap();

        let worker = Worker {
            cancel: Arc::new(AtomicBool::new(false)),
        };
        assert_eq!(worker.drain(app).await.unwrap(), 0);
        assert_eq!(queue_state(&app.db, "01A"), QueueState::Pending);
    }

    /// 큐가 비어 있으면 클라이언트도 만들지 않는다.
    #[tokio::test]
    async fn drain_on_empty_queue_is_zero() {
        let fx = temp_app("worker-empty");
        let app = &fx.app;
        let worker = Worker {
            cancel: Arc::new(AtomicBool::new(false)),
        };
        assert_eq!(worker.drain(app).await.unwrap(), 0);
    }

    /// 취소 상태로 들어오면 큐가 차 있어도 아무것도 하지 않는다 (§20.1).
    #[tokio::test]
    async fn drain_returns_immediately_when_cancelled() {
        let fx = temp_app("worker-cancelled");
        let app = &fx.app;
        app.db.queue_push("01A").unwrap();
        let worker = Worker {
            cancel: Arc::new(AtomicBool::new(true)),
        };
        assert_eq!(worker.drain(app).await.unwrap(), 0);
        assert_eq!(queue_state(&app.db, "01A"), QueueState::Pending);
    }

    // ───────────────────────────────────────────── T52 — 근거 없음

    /// 근거를 못 찾으면 **관측을 만들지 않고** 큐에만 실패로 남는다.
    #[test]
    fn no_ground_records_no_observation() {
        let fx = temp_app("worker-noground");
        let app = &fx.app;
        app.db.queue_push("01A").unwrap();
        let item = app.db.queue_claim().unwrap().unwrap();

        let before = soul_core::git::log_messages(&app.paths.soul(), 20)
            .unwrap()
            .len();
        let counted = settle(app, &item, JobOutcome::NoGround).unwrap();

        assert!(!counted);
        assert_eq!(
            app.store.count().unwrap(),
            0,
            "T52 — 추측 서술을 남기지 않는다"
        );
        assert_eq!(
            soul_core::git::log_messages(&app.paths.soul(), 20)
                .unwrap()
                .len(),
            before,
            "커밋도 생기지 않는다"
        );
        let entry = queue_item(&app.db, "01A").unwrap().unwrap();
        assert_eq!(
            entry.state,
            QueueState::Failed,
            "재시도로 풀리지 않는 실패다"
        );
        assert_eq!(entry.attempts, MAX_ATTEMPTS);
        assert_eq!(entry.last_error.as_deref(), Some(NO_GROUND));
    }

    /// API 실패는 3회까지 재시도된다 (§9.10) — 한 번의 실패로 굳지 않는다.
    #[test]
    fn api_failure_keeps_retry_budget() {
        let fx = temp_app("worker-apifail");
        let app = &fx.app;
        app.db.queue_push("01A").unwrap();
        let item = app.db.queue_claim().unwrap().unwrap();

        settle(app, &item, JobOutcome::Failed("429".into())).unwrap();
        let entry = queue_item(&app.db, "01A").unwrap().unwrap();
        assert_eq!(entry.state, QueueState::Pending);
        assert_eq!(entry.attempts, 1);
        assert_eq!(entry.last_error.as_deref(), Some("429"));
    }

    /// 성공하면 관측 1건 + 커밋 1개(`context <ULID>`), 큐는 `done`이다 (§R8).
    #[test]
    fn success_writes_one_observation_and_one_commit() {
        let fx = temp_app("worker-success");
        let app = &fx.app;
        app.db.queue_push("01A").unwrap();
        let item = app.db.queue_claim().unwrap().unwrap();
        let ctx = context_obs(1, id(9));

        let before = soul_core::git::log_messages(&app.paths.soul(), 20).unwrap();
        let counted = settle(app, &item, JobOutcome::Context(Box::new(ctx))).unwrap();

        assert!(counted);
        assert_eq!(app.store.count().unwrap(), 1);
        let log = soul_core::git::log_messages(&app.paths.soul(), 20).unwrap();
        assert_eq!(log.len(), before.len() + 1, "쓰기 1회 = 커밋 1개");
        assert_eq!(log[0], format!("context {}", id(1)));
        assert_eq!(queue_state(&app.db, "01A"), QueueState::Done);
        // §R8 — `context`는 재렌더 계기가 아니다. SOUL.md는 스켈레톤 그대로다.
        assert!(!log[0].starts_with("render"));
    }

    // ───────────────────────────────────────────── §10.4 추가 입력

    #[test]
    fn extra_input_comes_from_origin_only() {
        let src = |origin: &str| Source {
            kind: soul_core::obs::Kind::Image,
            sha256: "a".repeat(64),
            origin: origin.to_string(),
            bytes: 1,
            mime: "image/jpeg".into(),
        };
        assert_eq!(
            extra_from_source(&src("file:///Users/x/사진 01.jpg")).as_deref(),
            Some("사진 01.jpg"),
            "로컬 파일은 파일명 (§10.4)"
        );
        assert_eq!(
            extra_from_source(&src("https://www.youtube.com/watch?v=abc")).as_deref(),
            Some("https://www.youtube.com/watch?v=abc"),
            "YouTube는 URL을 그대로 넘겨 에이전트가 제목·채널을 읽게 한다"
        );
        assert_eq!(
            extra_from_source(&src("clipboard:9f2c1a3b4d5e")),
            None,
            "붙여넣은 텍스트의 원문은 남아 있지 않다 (T42)"
        );
    }

    #[test]
    fn critique_input_rejects_non_ingest_targets() {
        let fx = temp_app("worker-notingest");
        let app = &fx.app;
        let obs = Observation::Context(context_obs(1, id(9)));
        app.store.append(&obs).unwrap();
        let err = critique_input(app, id(1).as_str()).unwrap_err();
        assert!(matches!(err, SoulError::Invalid(_)), "{err}");
    }

    #[test]
    fn critique_input_reports_missing_observation() {
        let fx = temp_app("worker-missing");
        let app = &fx.app;
        let err = critique_input(app, id(3).as_str()).unwrap_err();
        assert!(matches!(err, SoulError::ObsNotFound(_)), "{err}");
    }

    // ───────────────────────────────────────────── run_one

    /// §9.10 비활성화 상태에서는 명령이어도 만들지 않는다.
    #[tokio::test]
    async fn run_one_refuses_when_context_is_disabled() {
        let mut fx = temp_app("worker-runone-disabled");
        fx.app.config.thresholds.context_enabled = false;
        let app = &fx.app;
        let err = run_one(app, &id(1)).await.unwrap_err();
        assert!(matches!(err, SoulError::Config(_)), "{err}");
    }

    /// 워커가 집어간 건은 겹쳐 처리하지 않는다.
    #[tokio::test]
    async fn run_one_refuses_while_the_worker_holds_the_item() {
        let fx = temp_app("worker-runone-running");
        let app = &fx.app;
        app.db.queue_push(id(1).as_str()).unwrap();
        app.db.queue_claim().unwrap().unwrap();
        let err = run_one(app, &id(1)).await.unwrap_err();
        assert!(matches!(err, SoulError::Invalid(_)), "{err}");
    }
}
