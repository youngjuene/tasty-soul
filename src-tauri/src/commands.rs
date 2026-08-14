//! Tauri 커맨드 — 프런트가 부를 수 있는 것의 전부 (§13).
//!
//! 이 목록이 프런트엔드 API의 계약이다. `src/lib/api.ts`가 이것을 그대로 미러링한다.
//!
//! ## 노출하지 않는 것
//!
//! - API 키 값 (§2). `secrets_status`가 불리언만 준다
//! - 파일 시스템 임의 접근. 경로는 다이얼로그와 드롭 이벤트에서만 온다
//! - `SOUL.md` 파일 통째 읽기를 원격으로 보내는 경로 (§D4)

use serde::{Deserialize, Serialize};

// ───────────────────────────────────────────── 설정 · 진단 (§9.9 · §D7)

#[derive(Serialize, Deserialize)]
pub struct SetupStatus {
    pub first_run: bool,
    /// §D7 — 첫 투입 전에 §D2 표를 보여야 하는가.
    pub needs_boundary_notice: bool,
    pub api_key_set: bool,
    pub models_unset: bool,
    pub context_enabled: bool,
}

/// 키체인 계정. 프런트의 `Setup.tsx: KNOWN_ACCOUNTS`와 **순서까지** 같다.
const SECRET_ACCOUNTS: [&str; 3] = [
    soul_net::secrets::ACCOUNT_OPENAI,
    soul_net::secrets::ACCOUNT_SEARCH,
    soul_net::secrets::ACCOUNT_YOUTUBE,
];

/// §D7 — 고지를 켜 두었고 아직 확인하지 않았으면 첫 투입 전에 §D2 표가 먼저다.
///
/// **확인 여부만으로 판단하지 않는다.** `show_boundary_on_first_run = false`로 꺼 둔
/// 사용자에게 고지를 다시 띄우면 설정이 무시된 것이 된다 (§9.8).
fn needs_boundary_notice(cfg: &soul_core::config::Config) -> bool {
    cfg.privacy.show_boundary_on_first_run && !cfg.privacy.boundary_acknowledged
}

/// 셸이 가장 먼저 부르는 커맨드다 (§13 · `App.tsx`).
///
/// **`AppState.app` 잠금을 잡지 않는다.** 무거운 작업(투입·재빌드·비평 워커)이 도는 동안에도
/// 창이 "준비 중"에서 멈추면 안 된다. `config.toml`을 그대로 읽는다 — 쓰는 경로
/// (`set_config`·`acknowledge_boundary`)가 파일과 메모리를 항상 함께 갱신하므로 두 값은 같다.
#[tauri::command]
pub async fn setup_status(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<SetupStatus, String> {
    // §3 — 열기에 실패했어도 창은 떠 있다. 일시적 실패였을 수 있으니 여기서 한 번 더
    // 열어 보고, 그래도 안 되면 사유를 그대로 올린다 (`App.tsx`가 «다시 시도»를 띄운다).
    let failed = state
        .open_error
        .lock()
        .map(|g| g.is_some())
        .unwrap_or(false);
    if failed {
        drop(pipeline_support::app_lock(&state).await?);
        if let Ok(mut slot) = state.open_error.lock() {
            *slot = None;
        }
    }

    let paths = soul_core::paths::Paths::discover().map_err(|e| e.to_string())?;
    let cfg = soul_core::config::Config::load(&paths.config_toml()).map_err(|e| e.to_string())?;

    Ok(SetupStatus {
        first_run: state.first_run.load(std::sync::atomic::Ordering::Relaxed),
        needs_boundary_notice: needs_boundary_notice(&cfg),
        // §2 — 값이 아니라 존재 여부만 본다.
        api_key_set: soul_net::secrets::is_set(soul_net::secrets::ACCOUNT_OPENAI),
        // §9.9 — 네 슬롯이 전부 비어 있으면 아무 투입도 처리할 수 없다 (§15).
        models_unset: cfg.models_unset(),
        context_enabled: cfg.thresholds.context_enabled,
    })
}

/// §D7 — 고지 확인. `context_enabled` 선택을 **함께 받는다.**
#[tauri::command]
pub async fn acknowledge_boundary(
    state: tauri::State<'_, crate::state::AppState>,
    context_enabled: bool,
) -> Result<(), String> {
    let mut guard = pipeline_support::app_lock(&state).await?;
    let app = guard
        .as_mut()
        .ok_or_else(|| "앱 상태가 아직 열리지 않았습니다".to_string())?;

    let mut cfg = app.config.clone();
    // 고지 확인과 문화 층 선택은 한 번에 저장한다 — 둘을 쪼개면 중간에 죽었을 때
    // 확인은 받았는데 선택은 반영되지 않은 상태가 남는다.
    cfg.thresholds.context_enabled = context_enabled;
    cfg.privacy.boundary_acknowledged = true;
    cfg.save(&app.paths.config_toml())
        .map_err(|e| e.to_string())?;
    // 파일과 메모리가 갈라지면 워커가 옛 값으로 검색을 낸다 (§9.10 · T54).
    app.config = cfg;
    Ok(())
}

/// 키를 키체인에 저장한다. **읽는 커맨드는 없다.**
#[tauri::command]
pub async fn set_secret(account: String, value: String) -> Result<(), String> {
    // 알려진 계정만 받는다. 임의 문자열을 그대로 넘기면 앱이 지울 방법이 없는 항목이
    // 키체인에 쌓이고, 사용자는 그것이 무엇인지 알 수 없다.
    if !SECRET_ACCOUNTS.contains(&account.as_str()) {
        return Err(format!("알 수 없는 키 계정입니다: {account}"));
    }
    // §2 — **`value`를 오류 문구·로그·트레이스 어디에도 넣지 않는다.**
    // `secrets::set`이 앞뒤 공백을 떼고 빈 값을 거절한다.
    soul_net::secrets::set(&account, &value).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn secrets_status() -> Result<Vec<(String, bool)>, String> {
    // §2 — 불리언까지다. 값을 돌려주는 경로는 이 파일 어디에도 없다.
    Ok(SECRET_ACCOUNTS
        .iter()
        .map(|a| ((*a).to_string(), soul_net::secrets::is_set(a)))
        .collect())
}

/// §9.9 — `probe_models = false`는 로컬 점검만 한다 (네트워크를 쓰지 않는다).
#[tauri::command]
pub async fn doctor(
    state: tauri::State<'_, crate::state::AppState>,
    probe_models: bool,
) -> Result<soul_pipeline::doctor::DoctorReport, String> {
    let guard = pipeline_support::app_lock(&state).await?;
    let app = pipeline_support::borrow(&guard)?;
    // `&App`은 `Send`가 아니다. `run_pipeline`이 그 사실을 커맨드 밖으로 새지 않게 한다.
    crate::state::run_pipeline(soul_pipeline::doctor::run(app, probe_models))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_config(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<soul_core::config::Config, String> {
    let guard = pipeline_support::app_lock(&state).await?;
    let app = pipeline_support::borrow(&guard)?;
    Ok(app.config.clone())
}

#[tauri::command]
pub async fn set_config(
    state: tauri::State<'_, crate::state::AppState>,
    config: soul_core::config::Config,
) -> Result<(), String> {
    let mut guard = pipeline_support::app_lock(&state).await?;
    let app = guard
        .as_mut()
        .ok_or_else(|| "앱 상태가 아직 열리지 않았습니다".to_string())?;

    // 파일이 먼저다. 쓰기에 실패했는데 메모리만 바뀌면 다음 실행에 되돌아가고,
    // 그 사이 파이프라인은 저장되지 않은 값으로 돌게 된다.
    config
        .save(&app.paths.config_toml())
        .map_err(|e| e.to_string())?;
    app.config = config;
    Ok(())
}

// ───────────────────────────────────────────────────── 투입 (§13 화면 1)

/// 투입·응답 계열 커맨드가 공유하는 도우미 (§9).
///
/// 커맨드는 전부 `soul_pipeline`의 얇은 어댑터다. 판단은 저 아래에 있고
/// 여기서는 **상태 잠금·문자열 파싱·표시용 변환**만 한다 (§2).
mod pipeline_support {
    use tauri::{Emitter as _, Manager as _};

    /// §13 화면 1이 듣는 이벤트. `src/screens/Ingest.tsx`의 `PROGRESS_EVENT`와 같아야 한다.
    pub(super) const PROGRESS_EVENT: &str = "ingest://progress";

    /// 진행 단계 (§13 화면 1 — "프레임 추출 → 오디오 변환 → 서술 → 병합").
    #[derive(Clone, serde::Serialize)]
    pub(super) struct ProgressPayload {
        /// 어떤 작업의 단계인지 맞추는 열쇠. 클립보드 투입은 경로가 없어 `null`이다.
        pub path: Option<String>,
        /// `soul_pipeline::ingest::stage`의 문자열을 그대로 보낸다.
        /// 프런트의 표에 없는 값은 그대로 표시된다 (`Ingest.tsx: stageLabel`).
        pub stage: String,
    }

    /// 진행 표시가 실패해도 투입은 계속된다. 단계 알림은 부가 정보다.
    pub(super) fn emit_stage(handle: &tauri::AppHandle, path: Option<&str>, stage: &str) {
        let _ = handle.emit(
            PROGRESS_EVENT,
            ProgressPayload {
                path: path.map(str::to_string),
                stage: stage.to_string(),
            },
        );
    }

    /// `AppState`의 `App`을 빌린다. 아직 열려 있지 않으면 §3 절차로 연다.
    ///
    /// 잠금은 `tokio::sync::Mutex`이므로 await를 건너 들고 있어도 된다 —
    /// 커맨드는 async 런타임에서 돌기 때문에 UI 스레드를 막지 않는다.
    pub(super) async fn app_lock<'a>(
        state: &'a crate::state::AppState,
    ) -> Result<tokio::sync::MutexGuard<'a, Option<soul_pipeline::App>>, String> {
        let mut guard = state.app.lock().await;
        if guard.is_none() {
            let paths = soul_core::paths::Paths::discover().map_err(|e| e.to_string())?;
            *guard = Some(soul_pipeline::App::open_or_init(paths).map_err(|e| e.to_string())?);
        }
        Ok(guard)
    }

    /// `app_lock`이 채워 두므로 언제나 `Some`이다. 그래도 패닉 대신 사유를 돌려준다.
    pub(super) fn borrow<'a>(
        guard: &'a tokio::sync::MutexGuard<'_, Option<soul_pipeline::App>>,
    ) -> Result<&'a soul_pipeline::App, String> {
        guard
            .as_ref()
            .ok_or_else(|| "앱 상태가 아직 열리지 않았습니다".to_string())
    }

    /// `AppHandle`에서 앱 상태를 꺼낸다. 셸이 `.manage()` 하지 않았으면 사유를 돌려준다.
    pub(super) fn state_of(
        handle: &tauri::AppHandle,
    ) -> Result<tauri::State<'_, crate::state::AppState>, String> {
        handle
            .try_state::<crate::state::AppState>()
            .ok_or_else(|| "앱 상태가 등록되지 않았습니다".to_string())
    }

    /// **`&App`이 필요하면서 `await`가 있는** 커맨드 본문을 전용 스레드에서 끝내고
    /// 결과만 받아 온다.
    ///
    /// Tauri v2의 async 커맨드는 `Send` future만 받는다 —
    /// `generate_handler!`가 커맨드를 `async_runtime::spawn` 하기 때문이다.
    /// 그런데 `App`은 sqlite 연결(`RefCell`)을 쥐고 있어 `Sync`가 아니고,
    /// 따라서 `&App`을 await 너머로 들고 있는 future는 `Send`가 아니다.
    /// 잠금부터 파이프라인 호출까지 **전부 이 스레드 안에서** 끝내고 결과만 건넨다.
    ///
    /// UI 스레드도, 커맨드용 async 런타임도 막지 않는다. 커맨드가 중간에 취소되어도
    /// 스레드는 하던 일을 끝낸다 — 관측은 이미 기록되는 중이므로 중도 포기가 더 나쁘다.
    ///
    /// **`crate::state::run_pipeline`과 목적이 같다.** 그쪽은 `block_in_place`로 지금
    /// 워커 스레드에서 future를 끝내므로 짧은 호출에 맞고, 이쪽은 20~60초짜리 투입·비평을
    /// 위해 런타임 워커를 그만큼 붙들지 않는다 (§9.1의 처리 시간 표).
    /// 한 커맨드 안에서 둘을 겹쳐 쓰지 말 것 — 이 스레드는 current-thread 런타임이라
    /// 그 안에서 `block_in_place`를 부르면 패닉한다.
    ///
    /// ```ignore
    /// let card = pipeline_support::on_app!(&app_handle, |app| {
    ///     let out = soul_pipeline::ingest::ingest(app, input, None)
    ///         .await
    ///         .map_err(|e| e.to_string())?;
    ///     Ok(pipeline_support::sensory_card(&out))
    /// })?;
    /// ```
    macro_rules! on_app {
        ($handle:expr, |$app:ident| $body:block) => {{
            let handle: tauri::AppHandle = ::std::clone::Clone::clone($handle);
            let (tx, rx) = ::tokio::sync::oneshot::channel();
            ::std::thread::spawn(move || {
                let out = match ::tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt.block_on(async move {
                        let state = crate::commands::pipeline_support::state_of(&handle)?;
                        let guard = crate::commands::pipeline_support::app_lock(&state).await?;
                        let $app = crate::commands::pipeline_support::borrow(&guard)?;
                        $body
                    }),
                    Err(e) => Err(e.to_string()),
                };
                // 받는 쪽이 사라졌으면 결과를 버린다. 작업 자체는 이미 끝났다.
                let _ = tx.send(out);
            });
            match rx.await {
                Ok(v) => v,
                Err(_) => Err("작업 스레드가 먼저 끝났습니다".to_string()),
            }
        }};
    }
    pub(super) use on_app;

    /// §20.4 — 썸네일은 base64 data URL로 준다. asset 프로토콜을 쓰지 않는다.
    /// 썸네일이 없거나 못 읽으면 `None`이다 (프런트가 서술문 앞 40자를 그린다, T70c).
    pub(super) fn thumb_data_url(path: &std::path::Path) -> Option<String> {
        let bytes = std::fs::read(path).ok()?;
        (!bytes.is_empty()).then(|| soul_media::image_in::to_data_url(&bytes))
    }

    /// `ingest` ID로 썸네일을 찾는다. 파일 키는 `source.sha256`이다 (§20.4).
    pub(super) fn thumb_for_ingest(
        app: &soul_pipeline::App,
        id: &soul_core::ids::ObsId,
    ) -> Option<String> {
        let obs = app.store.read(id).ok()?;
        let sha = &obs.as_ingest()?.source.sha256;
        thumb_data_url(&app.paths.thumb_file(sha))
    }

    pub(super) fn sensory_card(out: &soul_pipeline::ingest::IngestOutcome) -> super::SensoryCard {
        super::SensoryCard {
            ingest_id: out.id.to_string(),
            prose: out.prose.clone(),
            thumb_data_url: out.thumbnail.as_deref().and_then(thumb_data_url),
            kind: out.kind.as_str().to_string(),
            kind_is_guess: out.kind_is_guess,
        }
    }

    /// 미응답 감각 카드는 **영속화하지 않는다** (§13 화면 2). 세션 메모리에만 둔다.
    pub(super) async fn remember_card(state: &crate::state::AppState, card: &super::SensoryCard) {
        let mut cards = state.sensory_cards.lock().await;
        match cards.iter().position(|c| c.ingest_id == card.ingest_id) {
            Some(i) => cards[i] = card.clone(),
            None => cards.push(card.clone()),
        }
    }

    /// §9.3 — 뒤집기로 supersede된 카드를 새 카드로 갈아 끼운다.
    pub(super) async fn replace_card(
        state: &crate::state::AppState,
        old_ingest_id: &str,
        card: &super::SensoryCard,
    ) {
        let mut cards = state.sensory_cards.lock().await;
        match cards.iter().position(|c| c.ingest_id == old_ingest_id) {
            Some(i) => cards[i] = card.clone(),
            None => cards.push(card.clone()),
        }
    }

    /// §9.10 — 투입 직후 비평 워커를 깨운다.
    ///
    /// **건별 알림을 띄우지 않는다** (T51e). 결과도 기다리지 않는다 —
    /// 문화 글귀 실패가 투입 자체를 실패시키지 않기 때문이다 (T51).
    /// 앱 상태 잠금은 이 워커가 나중에 잡으므로, 부르는 쪽이 잠금을 놓은 뒤에 돈다.
    ///
    /// **전용 스레드에서 돈다.** `App`은 sqlite 연결(`RefCell`)을 쥐고 있어 `Sync`가
    /// 아니므로 `&App`을 들고 가는 future는 `Send`가 아니다 — 공유 런타임에
    /// `spawn` 할 수 없다. 상주 프로세스를 만드는 것이 아니라(§20.1) 앱 프로세스 안의
    /// 스레드이며, 취소 신호를 받으면 멈춘다 (T44).
    pub(super) fn wake_critique_worker(handle: &tauri::AppHandle) {
        let handle = handle.clone();
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                // 워커를 못 띄워도 투입은 이미 끝났다. 큐는 다음 기회에 이어받는다 (T51c).
                Err(_) => return,
            };
            rt.block_on(async move {
                let Some(state) = handle.try_state::<crate::state::AppState>() else {
                    return;
                };
                let Ok(guard) = app_lock(&state).await else {
                    return;
                };
                let Ok(app) = borrow(&guard) else { return };
                let worker = soul_pipeline::critique_worker::Worker {
                    // §20.1 · T44 — 앱을 닫으면 이 신호로 함께 멈춘다.
                    cancel: std::sync::Arc::clone(&state.cancel),
                };
                // 실패는 큐와 트레이스에 남는다 (§9.10). 여기서 팝업을 띄우지 않는다.
                let _ = worker.drain(app).await;
            });
        });
    }

    /// 실패 사유에 붙일 항목 이름. 경로 전체는 길어서 목록에서 읽히지 않는다.
    pub(super) fn file_label(path: &str) -> &str {
        path.rsplit(['/', '\\'])
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(path)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// 화면 1이 듣는 이름과 여기서 보내는 이름은 **같은 문자열**이어야 한다.
        /// 한쪽만 바꾸면 아무것도 실패하지 않고 진행 표시만 조용히 죽는다.
        #[test]
        fn progress_event_name_matches_the_screen() {
            let screen = include_str!("../../ui/screens/Ingest.tsx");
            assert!(
                screen.contains(&format!("\"{PROGRESS_EVENT}\"")),
                "Ingest.tsx가 {PROGRESS_EVENT} 를 듣지 않는다"
            );
            // Tauri v2의 이벤트 이름 규칙 (`is_event_name_valid`).
            assert!(PROGRESS_EVENT
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '-' | '/' | ':' | '_')));
        }

        /// §20.4 — 썸네일이 없으면 `None`이다. 빈 파일도 없는 것으로 친다
        /// (프런트가 서술문 앞 40자를 대신 그린다, T70c).
        #[test]
        fn thumbnails_come_back_as_data_urls_or_nothing() {
            let dir =
                std::env::temp_dir().join(format!("tasty-soul-thumb-{}", soul_core::ids::new_id()));
            std::fs::create_dir_all(&dir).unwrap();
            assert_eq!(thumb_data_url(&dir.join("없다.jpg")), None);

            let empty = dir.join("빈.jpg");
            std::fs::write(&empty, b"").unwrap();
            assert_eq!(thumb_data_url(&empty), None);

            let some = dir.join("있다.jpg");
            std::fs::write(&some, b"foobar").unwrap();
            assert_eq!(
                thumb_data_url(&some).as_deref(),
                Some("data:image/jpeg;base64,Zm9vYmFy"),
                "asset 프로토콜이 아니라 base64 data URL이다"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }

        /// `on_app!`와 `wake_critique_worker`는 전용 스레드에 current-thread 런타임을
        /// 세우고 그 위에서 **네트워크를 쓰는** 파이프라인을 돌린다.
        /// `enable_all()`은 tokio에 `net` 기능이 켜져 있을 때만 io 드라이버를 켜므로,
        /// 그 조건이 사라지면 런타임에 "there is no reactor running"으로 죽는다.
        /// 여기서 컴파일 시점에 못박는다.
        #[test]
        fn worker_thread_runtime_gets_io_and_time() {
            // 이 타입이 있으면 `net`이 켜져 있다 (reqwest가 켠다).
            fn _needs_the_io_driver(_: Option<tokio::net::TcpStream>) {}
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("워커 스레드 런타임");
            // future는 런타임 **안에서** 만들어야 한다. 밖에서 만들면 타이머가 등록되지
            // 않는다 — `on_app!`이 본문 전체를 `block_on` 안에 두는 이유이기도 하다.
            rt.block_on(async { tokio::time::sleep(std::time::Duration::from_millis(1)).await });
        }

        #[test]
        fn file_label_is_the_last_segment() {
            assert_eq!(file_label("/a/b/사진.jpg"), "사진.jpg");
            assert_eq!(file_label(r"C:\a\b\사진.jpg"), "사진.jpg");
            assert_eq!(file_label("사진.jpg"), "사진.jpg");
            // 끝이 구분자면 자를 것이 없다. 원문을 그대로 쓴다.
            assert_eq!(file_label("/a/b/"), "/a/b/");
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct SensoryCard {
    pub ingest_id: String,
    pub prose: String,
    pub thumb_data_url: Option<String>,
    pub kind: String,
    /// §9.3 — YouTube 추정 결과. true면 한 탭으로 뒤집을 수 있다.
    pub kind_is_guess: bool,
}

/// 드래그앤드롭은 **파일 경로만** 준다 (§13 화면 1).
///
/// **한 건이 실패해도 나머지는 진행한다.** 파일 하나 때문에 배치 전체가 죽지 않는다.
/// 실패 사유는 그 항목의 진행 표시에 남기고, 전부 실패했을 때만 에러로 돌려준다
/// — 반환 타입에 성공 카드 말고 다른 자리가 없기 때문이다.
#[tauri::command]
pub async fn ingest_files(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, crate::state::AppState>,
    paths: Vec<String>,
) -> Result<Vec<SensoryCard>, String> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let emitter = app_handle.clone();

    let cards: Vec<SensoryCard> = pipeline_support::on_app!(&app_handle, |app| {
        use soul_pipeline::ingest::{self, IngestInput, ProgressFn};

        let mut cards: Vec<SensoryCard> = Vec::new();
        let mut failures: Vec<String> = Vec::new();
        for path in &paths {
            // 진행 단계는 경로와 함께 보낸다 — 프런트가 이 값으로 작업을 맞춘다.
            let handle = emitter.clone();
            let owned = path.clone();
            let progress = move |stage: &str| {
                pipeline_support::emit_stage(&handle, Some(&owned), stage);
            };
            let progress: ProgressFn<'_> = &progress;
            let input = IngestInput::File(std::path::PathBuf::from(path));
            match ingest::ingest(app, input, Some(progress)).await {
                Ok(out) => cards.push(pipeline_support::sensory_card(&out)),
                Err(e) => {
                    let reason = e.to_string();
                    // 남은 파일이 도는 동안 이 항목의 사유가 화면에 보인다.
                    pipeline_support::emit_stage(&emitter, Some(path), &format!("실패 — {reason}"));
                    failures.push(format!("{}: {reason}", pipeline_support::file_label(path)));
                }
            }
        }
        // 전부 실패했을 때만 에러다. 한 장이라도 나오면 그것이 결과다.
        if cards.is_empty() && !failures.is_empty() {
            return Err(failures.join("\n"));
        }
        Ok(cards)
    })?;

    if cards.is_empty() {
        return Ok(cards);
    }
    for card in &cards {
        pipeline_support::remember_card(&state, card).await;
    }
    // §9.10 — 큐에 들어간 것을 바로 처리하기 시작한다. 건별 알림은 없다 (T51e).
    pipeline_support::wake_critique_worker(&app_handle);
    Ok(cards)
}

/// 클립보드 투입 (`Cmd/Ctrl+Shift+V`). URL이면 YouTube만 받는다 (§9.1, T11c).
///
/// §9.1 단계 1~3(YouTube · 그 외 URL 거부 · 텍스트)의 판정은 `soul-media::probe`에 있다.
/// 거절 문구를 여기서 다시 쓰지 않는다 — 두 곳에 두면 갈라진다.
#[tauri::command]
pub async fn ingest_clipboard(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, crate::state::AppState>,
    text: String,
) -> Result<SensoryCard, String> {
    let emitter = app_handle.clone();

    let card = pipeline_support::on_app!(&app_handle, |app| {
        use soul_pipeline::ingest::{self, IngestInput, ProgressFn};

        // 클립보드 투입에는 경로가 없다. 진행 중인 작업이 하나뿐이면 프런트가 알아서 붙인다.
        let progress = move |stage: &str| pipeline_support::emit_stage(&emitter, None, stage);
        let progress: ProgressFn<'_> = &progress;
        let out = ingest::ingest(app, IngestInput::Text(text), Some(progress))
            .await
            .map_err(|e| e.to_string())?;
        Ok(pipeline_support::sensory_card(&out))
    })?;

    pipeline_support::remember_card(&state, &card).await;
    pipeline_support::wake_critique_worker(&app_handle);
    Ok(card)
}

/// §9.10 — 화면 1에 조용히 표시할 대기 건수.
///
/// **아직 답을 받지 못한 문화 글귀의 수**다. 두 상태를 합친다:
/// 1. 큐에 남아 아직 만들어지지 않은 항목 (§9.10 "대기 중인 항목 수")
/// 2. 만들어졌지만 사용자가 아직 ○/×를 주지 않은 카드 (§13 화면 2.1 · T51e —
///    "문화 카드 5건 동시 도착 → 알림 1회, 대기 건수만 갱신")
///
/// 두 집합은 겹치지 않는다. `context` 관측이 생기는 순간 그 항목은 큐에서 `done`이 되고
/// 미응답 카드 쪽으로 옮겨 간다. 그래서 카드가 도착해도 이 수가 줄지 않고,
/// 사용자가 답할 때 비로소 줄어든다 — 화면 1의 "목록 열기"가 가리키는 수와 같아진다.
#[tauri::command]
pub async fn critique_pending_count(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<usize, String> {
    use soul_core::db::queue::QueueState;

    let guard = pipeline_support::app_lock(&state).await?;
    let app = pipeline_support::borrow(&guard)?;
    // T54 — 문화 층이 꺼져 있으면 대기라는 것 자체가 없다.
    if !app.config.thresholds.context_enabled {
        return Ok(0);
    }
    let queued = app.db.queue_pending_count().map_err(|e| e.to_string())?;
    // 지금 워커가 붙들고 있는 건도 사용자 눈에는 대기다.
    let running = app
        .db
        .queue_items(Some(QueueState::Running))
        .map_err(|e| e.to_string())?
        .len();
    let unanswered = soul_pipeline::reading::pending_cultural(app)
        .map_err(|e| e.to_string())?
        .len();
    Ok(queued + running + unanswered)
}

// ────────────────────────────────────────── ○/× 응답 (§13 화면 2 · 2.1)

/// `verdict`는 `"yes"` | `"no"` 뿐이다. 중간값을 추가하지 말 것 (T59).
#[tauri::command]
pub async fn record_reading(
    app_handle: tauri::AppHandle,
    target: String,
    layer: String,
    verdict: String,
    prose: Option<String>,
) -> Result<String, String> {
    use soul_core::obs::{Layer, Verdict};

    // 중간값을 받아 주는 순간 화면 2의 버튼이 셋이 된다 (T56 · T59).
    let verdict = Verdict::parse(&verdict).ok_or_else(|| {
        format!(
            "verdict는 \"yes\" 또는 \"no\" 뿐입니다 (중간 선택지를 두지 않는다, §6.3): {verdict}"
        )
    })?;
    let layer = Layer::parse(&layer)
        .ok_or_else(|| format!("layer는 \"sensory\" 또는 \"cultural\" 이어야 합니다: {layer}"))?;
    // §12.6 — cultural의 target은 `ingest`가 아니라 `context`다. 확인은 파이프라인이 한다.
    let target = soul_core::ids::ObsId::parse(&target).map_err(|e| e.to_string())?;

    // `no` + 문장이면 임베딩 호출이 붙는다 (§6.3). await가 있으므로 전용 스레드로 보낸다.
    pipeline_support::on_app!(&app_handle, |app| {
        soul_pipeline::reading::record(app, &target, layer, verdict, prose)
            .await
            .map(|id| id.to_string())
            .map_err(|e| e.to_string())
    })
}

#[derive(Serialize, Deserialize)]
pub struct CulturalCard {
    pub context_id: String,
    pub ingest_id: String,
    pub critique: String,
    pub lineage: Vec<String>,
    /// false면 카드 상단에 "근거를 충분히 찾지 못했습니다" (T58).
    pub grounded: bool,
    pub sources: Vec<SourceLink>,
    pub thumb_data_url: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct SourceLink {
    pub url: String,
    pub title: String,
}

/// §13 화면 2.1 — 미응답 문화 카드는 **앱 재시작 후에도 남는다** (T53).
#[tauri::command]
pub async fn pending_cultural_cards(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<Vec<CulturalCard>, String> {
    let guard = pipeline_support::app_lock(&state).await?;
    let app = pipeline_support::borrow(&guard)?;
    // 목록은 관측 로그에서 매번 다시 만든다. 그래서 앱을 다시 열어도 남아 있다 (T53).
    let pending = soul_pipeline::reading::pending_cultural(app).map_err(|e| e.to_string())?;
    Ok(pending
        .into_iter()
        .map(|c| CulturalCard {
            thumb_data_url: pipeline_support::thumb_for_ingest(app, &c.ingest_id),
            context_id: c.context_id.to_string(),
            ingest_id: c.ingest_id.to_string(),
            critique: c.critique,
            lineage: c.lineage,
            // §6.4 — 모델이 아니라 파이프라인이 센 값을 그대로 옮긴다 (T58).
            grounded: c.grounded,
            sources: c
                .sources
                .into_iter()
                .map(|s| SourceLink {
                    url: s.url,
                    title: s.title,
                })
                .collect(),
        })
        .collect())
}

/// §9.3 — kind 뒤집기. 버려질 응답 수를 먼저 확인시킨다.
#[tauri::command]
pub async fn recast_kind(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, crate::state::AppState>,
    ingest_id: String,
    kind: String,
) -> Result<SensoryCard, String> {
    let id = soul_core::ids::ObsId::parse(&ingest_id).map_err(|e| e.to_string())?;
    let kind = soul_core::obs::Kind::parse(&kind)
        .ok_or_else(|| format!("kind는 text·image·audio·video 중 하나여야 합니다: {kind}"))?;

    let card = pipeline_support::on_app!(&app_handle, |app| {
        // 뒤집기 가능 여부(YouTube 항목인가·같은 kind인가)는 파이프라인이 판정한다.
        let out = soul_pipeline::recast::recast(app, &id, kind)
            .await
            .map_err(|e| e.to_string())?;
        Ok(pipeline_support::sensory_card(&out))
    })?;

    // 이전 관측은 supersede되었다 (§R9). 세션 카드도 새것으로 갈아 끼운다.
    pipeline_support::replace_card(&state, &ingest_id, &card).await;
    // 새 ID로 §9.10 큐에 다시 들어갔다. 이전 `context`는 이월되지 않는다 (T11i).
    pipeline_support::wake_critique_worker(&app_handle);
    Ok(card)
}

#[tauri::command]
pub async fn recast_preview(
    state: tauri::State<'_, crate::state::AppState>,
    ingest_id: String,
) -> Result<usize, String> {
    let id = soul_core::ids::ObsId::parse(&ingest_id).map_err(|e| e.to_string())?;
    let guard = pipeline_support::app_lock(&state).await?;
    let app = pipeline_support::borrow(&guard)?;
    soul_pipeline::recast::discarded_readings(app, &id).map_err(|e| e.to_string())
}

/// §9.10 — 문화 글귀 재시도.
///
/// `soul context <id> --redo`와 같은 경로다. 검색 결과는 시간이 지나면 달라지므로
/// 나중에 다시 시도할 값어치가 있다. 근거를 못 찾으면 관측을 만들지 않으며,
/// 그것은 **에러가 아니라 정상적인 결과다** (T52) — 항목 상세가 다시 "문화 글귀 없음"을 보인다.
#[tauri::command]
pub async fn redo_context(app_handle: tauri::AppHandle, ingest_id: String) -> Result<(), String> {
    let id = soul_core::ids::ObsId::parse(&ingest_id).map_err(|e| e.to_string())?;
    // 검색 + 툴콜 루프라 20~60초다. 프런트는 끝난 뒤 상세를 다시 읽는다.
    pipeline_support::on_app!(&app_handle, |app| {
        soul_pipeline::critique_worker::run_one(app, &id)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
}

// ─────────────────────────────────────────────────── SOUL.md (§13 화면 3)

/// **로컬 표시용이므로 원문 그대로 준다** (§D4 — 목적지가 로컬이다).
///
/// `soulblocks`(목적지별 조립)를 거치지 않는다. 그 모듈은 **원격으로 나가는** 블록을
/// 고르는 곳이고, 여기 목적지는 이 기기의 화면 3이다. 사람이 쓴 `soul:human`을
/// 자기 화면에서 못 보게 만드는 것은 §D4가 막으려던 일이 아니다.
#[tauri::command]
pub async fn read_soul_md(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<String, String> {
    let guard = pipeline_support::app_lock(&state).await?;
    let app = pipeline_support::borrow(&guard)?;
    let path = app.paths.soul_md();
    std::fs::read_to_string(&path)
        .map_err(|e| format!("{} 을(를) 읽지 못했습니다: {e}", path.display()))
}

#[derive(Serialize, Deserialize)]
pub struct SaveResult {
    pub profile_edits: usize,
    pub commits: usize,
    /// §8.3 규칙 7 — "재빌드 시 덮어써집니다".
    pub gen_blocks_modified: Vec<String>,
}

/// §8.4 — 저장 시퀀스 전체는 `soulmd::save_edited`에 있다. 여기서 다시 쓰지 않는다.
///
/// 그 함수가 락 → 파싱 → `soul:human` 보관 → `profile_edit` 기록·커밋 → 재렌더 →
/// `render <T_ref>` 커밋을 **그 순서대로** 한다. 파싱 실패(마커 짝 불일치)는 파일을
/// 건드리지 않고 그대로 올라온다 — 편집 중인 내용이 사라지지 않는다 (T10).
#[tauri::command]
pub async fn save_soul_md(
    state: tauri::State<'_, crate::state::AppState>,
    text: String,
) -> Result<SaveResult, String> {
    let guard = pipeline_support::app_lock(&state).await?;
    let app = pipeline_support::borrow(&guard)?;

    // git 커밋이 여럿 생기는 경로다 (T21c). 워커 스레드로 옮겨 async 런타임을 막지 않는다.
    let outcome = archive_support::blocking(|| soul_core::soulmd::save_edited(&app.paths, &text))
        .map_err(|e| e.to_string())?;
    // 방금 `profile_edit` 관측이 늘었다. 인덱스를 비워야 대시보드가 같은 로그를 본다 (§R2).
    app.store.invalidate();

    Ok(SaveResult {
        profile_edits: outcome.profile_edits.len(),
        commits: outcome.commits,
        gen_blocks_modified: outcome.gen_blocks_modified,
    })
}

// ─────────────────────────────────────────────────── 성찰 (§13 화면 4)

#[derive(Serialize, Deserialize)]
pub struct ProposalView {
    /// 좌우 diff **표시**용 전문. `soul:human`이 들어 있다 (§8.2).
    ///
    /// 목적지가 로컬 화면이므로 §D4 대상이 아니다. 다만 **편집 상자에 넣지 말 것** —
    /// 그러면 사용자가 고친 전문이 승인 경로로 돌아와 관측에 실리고, `profile`은
    /// `soul:neg`이라 다음 성찰부터 원격으로 나간다 (§18-4·T29). 편집은 아래 두 필드로.
    pub current_text: String,
    pub proposed_text: String,
    /// 편집 상자가 바인딩하는 값 — `profile` 블록 본문**만**이다 (§D4).
    pub current_profile_text: String,
    pub proposed_profile_text: String,
    pub axis_delta: std::collections::BTreeMap<String, f64>,
    pub cites: Vec<String>,
    pub rationale: String,
}

/// §11.2 — 트리거를 만족할 때만 모델을 부른다. `force`는 그 판정을 무시한다.
///
/// 제안 원본(`SoulDelta`와 `SOUL.next.md` 전문)은 `AppState`에 남겨 둔다.
/// `approve_proposal`이 **같은 것**을 기록해야 `from_hash`가 제안 시점 값으로 남는다.
#[tauri::command]
pub async fn reflect(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, crate::state::AppState>,
    force: bool,
) -> Result<Option<ProposalView>, String> {
    // 에이전트 루프는 `&App`을 await 너머로 들고 간다 — 작업 스레드에서 끝내고
    // 결과만 받아 온다 (`on_app!`의 주석 참고).
    let proposal = pipeline_support::on_app!(&app_handle, |app| {
        soul_pipeline::reflect_flow::propose(app, force)
            .await
            .map_err(|e| e.to_string())
    })?;

    let mut slot = state.proposal.lock().await;
    match proposal {
        // 제안이 없으면 지난번 것을 남겨 두지 않는다 — 화면 4가 낡은 diff를 띄운다.
        None => {
            *slot = None;
            Ok(None)
        }
        Some(p) => {
            let view = archive_support::proposal_view(&p);
            *slot = Some(p);
            Ok(Some(view))
        }
    }
}

/// 승인. `modified_text`가 있으면 "수정 후 승인"이다 (§13 화면 4).
///
/// 커밋 둘이 생긴다: `soul_delta <ULID>` + `render <T_ref>` (§R8).
#[tauri::command]
pub async fn approve_proposal(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, crate::state::AppState>,
    modified_text: Option<String>,
) -> Result<String, String> {
    let mut slot = state.proposal.lock().await;
    // 제안을 꺼내 작업 스레드로 넘긴다. 잠금은 계속 쥔 채다 — 승인 도중에 들어온
    // `reflect`가 제안을 갈아 끼우면 사용자가 본 diff와 기록되는 것이 달라진다.
    let proposal = slot.take().ok_or_else(|| {
        // 앱을 다시 연 뒤의 승인이 여기로 온다. 제안을 새로 만들어 몰래 기록하지 않는다.
        "승인할 제안이 없습니다. `성찰 실행`으로 제안을 다시 받으십시오 (§13 화면 4)".to_string()
    })?;

    // 실패하면 제안을 제자리에 돌려놓는다 — 화면 4의 diff가 사라지면 사용자는
    // 무엇을 승인하려 했는지 다시 볼 방법이 없다 (§15 — 알리되 잃지 않는다).
    let outcome = pipeline_support::on_app!(&app_handle, |app| {
        match soul_pipeline::reflect_flow::approve(app, &proposal, modified_text.as_deref()).await {
            Ok(id) => Ok(Ok(id.to_string())),
            Err(e) => Ok(Err((e.to_string(), proposal))),
        }
    })?;

    match outcome {
        // 승인된 제안은 이미 `SOUL.md`에 있다. 두 번 기록되지 않도록 자리를 비워 둔다.
        Ok(id) => Ok(id),
        Err((reason, returned)) => {
            *slot = Some(returned);
            Err(reason)
        }
    }
}

/// 거절. **관측을 기록하지 않는다** (§11.2). 대기본만 사라진다.
#[tauri::command]
pub async fn reject_proposal(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<(), String> {
    {
        let guard = pipeline_support::app_lock(&state).await?;
        let app = pipeline_support::borrow(&guard)?;
        // 트레이스에만 남는다. 파일 하나를 지우는 일이라 작업 스레드가 필요 없다.
        soul_pipeline::reflect_flow::reject(app).map_err(|e| e.to_string())?;
    }
    *state.proposal.lock().await = None;
    Ok(())
}

// ────────────────────────────────────────────── 대시보드 (§13 화면 5)

/// **읽기만.** 계산 없음 (§20.7). 첫 렌더 500ms 예산 (T39).
///
/// `App::derived()`가 파생값의 **유일한** 진입점이다. 여기서 임베딩을 새로 만들지도,
/// 화면용 값을 따로 접지도 않는다 — 그러면 `SOUL.md`와 대시보드가 같은 로그를 두고
/// 다른 숫자를 말하게 된다 (§R2). 임베딩은 캐시에서만 읽으므로 네트워크를 쓰지 않는다.
#[tauri::command]
pub async fn dashboard(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<soul_core::derived::Derived, String> {
    let guard = pipeline_support::app_lock(&state).await?;
    let app = pipeline_support::borrow(&guard)?;
    // 관측 파일 읽기 + 파생 계산은 CPU·디스크 작업이다. 워커 스레드로 옮긴다 (T39).
    archive_support::blocking(|| app.derived()).map_err(|e| e.to_string())
}

// ───────────────────────────────────────────── 아카이브 (§13 화면 6)

#[derive(Serialize, Deserialize, Default)]
pub struct ArchiveQuery {
    pub kinds: Vec<String>,
    pub cells: Vec<String>,
    pub cluster: Option<usize>,
    pub surprisal_min: Option<f64>,
    pub surprisal_max: Option<f64>,
    pub months: Vec<String>,
    pub tags: Vec<String>,
    pub qualities: Vec<String>,
    /// 부분 문자열 일치만. **의미 검색은 없다** (§13 화면 6).
    pub search: Option<String>,
    /// 산점도 축.
    pub x_axis: Option<String>,
    pub y_axis: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct ArchiveItem {
    pub id: String,
    pub kind: String,
    /// 썸네일이 없으면 `None` — 프런트가 서술문 앞 40자를 렌더한다 (T70c).
    pub thumb_data_url: Option<String>,
    pub prose: String,
    pub tags: Vec<String>,
    pub surprisal: f64,
    pub quality: String,
    pub cell: Option<String>,
    pub cluster: Option<usize>,
    pub x: f64,
    pub y: f64,
    /// 구조 보기(PCA) 좌표.
    pub px: Option<f64>,
    pub py: Option<f64>,
    pub month: String,
}

/// **어떤 필터도 API를 호출하지 않는다** (T68).
///
/// - **supersede된 `ingest`는 나오지 않는다** (§R9, T70b) — `ObsSet::active_ingests()`가 관문이다
/// - 좌표 `x`/`y`는 `x_axis`/`y_axis`가 가리키는 **감각 축 값**이다 (기본 `grain` × `valence`)
/// - `px`/`py`는 PCA 좌표. `T_ref`의 **날짜**가 바뀔 때만 무효화한다 (§13 화면 6)
/// - 썸네일은 결과가 200건 이하일 때만 채운다 (T67). 초과하면 프런트가 점을 찍는다
#[tauri::command]
pub async fn archive_query(
    state: tauri::State<'_, crate::state::AppState>,
    query: ArchiveQuery,
) -> Result<Vec<ArchiveItem>, String> {
    let guard = pipeline_support::app_lock(&state).await?;
    let app = pipeline_support::borrow(&guard)?;
    archive_support::blocking(|| archive_support::run_query(app, &query))
}

/// 캐시된 벡터끼리 코사인. **API 호출 없음** (T33·T68).
///
/// `soul_similar`(§19.5)과 같은 연산이며 같은 필터를 쓴다 — 활성 ingest만, 주 공간
/// (`obs_vec`)만. 새 임베딩이 필요하면 만들지 않고 **사유를 말한다**.
#[tauri::command]
pub async fn archive_neighbors(
    state: tauri::State<'_, crate::state::AppState>,
    id: String,
    n: usize,
) -> Result<Vec<ArchiveItem>, String> {
    let guard = pipeline_support::app_lock(&state).await?;
    let app = pipeline_support::borrow(&guard)?;
    archive_support::blocking(|| archive_support::run_neighbors(app, &id, n))
}

#[derive(Serialize, Deserialize)]
pub struct ItemDetail {
    pub item: ArchiveItem,
    pub origin: String,
    pub sensory_prose: String,
    pub sensory_reading: Option<ReadingView>,
    pub context: Option<CulturalCard>,
    pub cultural_reading: Option<ReadingView>,
    /// §9.10 — 문화 글귀가 없는 항목은 그 사실을 표시하고 재시도 버튼을 둔다.
    pub context_failed: bool,
    pub can_recast: bool,
}

#[derive(Serialize, Deserialize)]
pub struct ReadingView {
    pub verdict: String,
    pub prose: Option<String>,
    pub divergence: Option<f64>,
}

/// 두 글귀를 나란히 놓는 데 필요한 것 전부 (§13 화면 6).
///
/// `sensory_reading`·`cultural_reading`이 `null`이면 **그 층은 아직 미응답**이다.
/// 프런트는 그 자리에서 ○/×를 받는다 — 화면 2·2.1을 다시 띄우지 않는다 (T70).
#[tauri::command]
pub async fn archive_detail(
    state: tauri::State<'_, crate::state::AppState>,
    id: String,
) -> Result<ItemDetail, String> {
    let guard = pipeline_support::app_lock(&state).await?;
    let app = pipeline_support::borrow(&guard)?;
    archive_support::blocking(|| archive_support::run_detail(app, &id))
}

// ─────────────────────────────────────────────────────── 기타 (§14 · §19.7)

/// §19.7 — `soul mcp --print-config`가 내는 것과 **같은 JSON**이다.
///
/// **설정 파일을 만들지도 고치지도 않는다** (T38). 클라이언트마다 파일 위치와 키 이름이
/// 다르므로 앱은 출력과 안내까지만 한다. 프런트가 복사 버튼과 함께 보여준다.
#[tauri::command]
pub async fn mcp_config_json() -> Result<String, String> {
    let v = serde_json::json!({
        "mcpServers": {
            "soul": {
                "command": "soul",
                "args": ["mcp"],
            }
        }
    });
    serde_json::to_string_pretty(&v).map_err(|e| e.to_string())
}

/// §19.8 — 축소 경로. `SOUL.md` 원문에서 **마커 주석만** 제거한 것을 돌려준다.
///
/// 번역·요약·축약을 하지 않는다. 조립은 전부 `soulblocks::export_prompt`가 한다 —
/// 여기서 문자열을 손대면 §19.8이 깨진다. `exports/SOUL.prompt.md`에도 남긴다
/// (`soul export --target=prompt`와 같은 자리다. `exports/`는 git 밖이다, §3).
#[tauri::command]
pub async fn export_prompt(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<String, String> {
    let guard = pipeline_support::app_lock(&state).await?;
    let app = pipeline_support::borrow(&guard)?;

    let md_path = app.paths.soul_md();
    let raw = std::fs::read_to_string(&md_path)
        .map_err(|e| format!("{} 을(를) 읽을 수 없습니다: {e}", md_path.display()))?;
    // §15 · T10 — 마커 짝이 맞지 않으면 아무것도 쓰지 않고 사유만 올린다.
    let doc = soul_core::soulmd::parse(&raw).map_err(|e| e.to_string())?;
    let out = soul_core::soulblocks::export_prompt(&doc).map_err(|e| e.to_string())?;

    let dir = app.paths.exports();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dest = dir.join("SOUL.prompt.md");
    std::fs::write(&dest, out.as_bytes())
        .map_err(|e| format!("{} 에 쓸 수 없습니다: {e}", dest.display()))?;
    Ok(out)
}

/// §11.3 · T71 — `runs/`만 비운다. **관측과 `SOUL.md`는 건드리지 않는다.**
///
/// `Tracer`는 쓸 때마다 append로 파일을 다시 열므로 다음 호출이 알아서 새로 만든다.
#[tauri::command]
pub async fn trace_purge(state: tauri::State<'_, crate::state::AppState>) -> Result<usize, String> {
    let guard = pipeline_support::app_lock(&state).await?;
    let app = pipeline_support::borrow(&guard)?;
    soul_core::trace::purge(&app.paths).map_err(|e| e.to_string())
}

/// §14 — `soul rebuild [--from-scratch] [--offline]`과 **같은 것**을 한다.
///
/// 반환 문자열은 CLI가 stdout·stderr에 내던 요약을 줄바꿈으로 이은 것이다.
#[tauri::command]
pub async fn rebuild(
    state: tauri::State<'_, crate::state::AppState>,
    from_scratch: bool,
    offline: bool,
) -> Result<String, String> {
    // 재빌드가 도는 동안 다른 커맨드가 같은 로그에 관측을 더하면 재생 결과가 흔들린다.
    // 잠금을 끝까지 쥔다 (§R2).
    let guard = pipeline_support::app_lock(&state).await?;
    let app = pipeline_support::borrow(&guard)?;
    crate::state::run_pipeline(rebuild_support::run(app, from_scratch, offline))
}

/// §14 표 · §R2 · §R3 · §R8 — `soul-cli::cmd::rebuild::rebuild`와 **같은 순서**다.
///
/// | 명령 | derived.sqlite | SOUL.md | 커밋 |
/// |---|---|---|---|
/// | `rebuild` | 관측 재생으로 갱신 | 재작성 | `rebuild <n>` |
/// | `rebuild --from-scratch` | 파생 테이블만 비우고 전량 재구축 | 재작성 | `rebuild <n>` |
///
/// **두 경로가 어긋나면 같은 로그에서 서로 다른 `SOUL.md`가 나온다** — 아무것도 실패하지
/// 않으므로 눈치채지 못한다. 순서를 바꾸기 전에 `crates/soul-cli/src/cmd/rebuild.rs`를 본다.
mod rebuild_support {
    use soul_core::db::embed_cache::Space;
    use soul_core::derived::Derived;
    use soul_core::obs::Observation;
    use soul_core::paths::Paths;
    use soul_core::rebuild as core_rebuild;
    use soul_core::soulmd::NULL_GLYPH;
    use std::collections::HashSet;

    pub(super) async fn run(
        app: &soul_pipeline::App,
        from_scratch: bool,
        offline: bool,
    ) -> Result<String, String> {
        let paths = &app.paths;
        let mut lines: Vec<String> = Vec::new();

        // 1. `--from-scratch`는 **파생 테이블만** 비운다.
        //    `derived.sqlite` 파일을 지우면 임베딩 캐시가 함께 사라져 T2가 깨진다.
        if from_scratch {
            app.db.clear_derived().map_err(|e| e.to_string())?;
            let cached = app.db.embed_count().map_err(|e| e.to_string())?;
            lines.push(format!(
                "파생 테이블을 비웠습니다 (임베딩 캐시 {cached}건은 보존, T2)."
            ));
        }

        // 2. 1차 재생. `offline`이면 캐시 미스에서 그대로 에러 종료한다 (T28).
        let (derived, missing) = replay(paths, offline)?;

        // 3. §R3 — 재현성의 유일한 네트워크 예외. 채운 뒤 **다시 재생**한다.
        let derived = if missing.is_empty() {
            derived
        } else {
            lines.push(format!(
                "임베딩 캐시 미스 {}건 — 새로 계산합니다 (§R3)",
                missing.len()
            ));
            let filled = backfill(app, &missing).await?;
            lines.push(format!("임베딩 {filled}건을 캐시에 넣었습니다."));
            let (derived, still_missing) = replay(paths, false)?;
            if !still_missing.is_empty() {
                return Err(format!(
                    "임베딩을 채운 뒤에도 캐시 미스가 {}건 남았습니다",
                    still_missing.len()
                ));
            }
            derived
        };

        // 4. §R8 — 커밋 메시지는 `rebuild <관측 수>`. 로그 전체 건수다.
        let n = derived.total_observation_count;
        let message = format!("rebuild {n}");
        let report =
            core_rebuild::render_soul_md(paths, &derived, &message).map_err(|e| e.to_string())?;

        // 방금 `SOUL.md`가 다시 쓰였다. 다음 읽기가 같은 로그를 보도록 인덱스를 비운다.
        app.store.invalidate();

        for w in &report.warnings {
            lines.push(format!("경고: {w}"));
        }
        lines.push(format!(
            "관측 {}건 (활성 ingest {}건) · 기준 {}",
            report.observations,
            derived.observation_count,
            t_ref_text(&derived)
        ));
        lines.push(format!(
            "{} — {}",
            paths.soul_md().display(),
            if report.soul_md_changed {
                "재작성"
            } else {
                "변경 없음"
            }
        ));
        lines.push(match &report.commit {
            Some(c) => format!("커밋 {}", &c[..c.len().min(12)]),
            None => "커밋 없음 (변경 없음)".to_string(),
        });
        Ok(lines.join("\n"))
    }

    /// 재생은 관측 전량을 읽고 군집까지 돌린다. 무겁지만 여기는 이미 `run_pipeline`이
    /// 떼어 놓은 블로킹 스레드 위이므로 그냥 부른다 (`block_in_place`를 중첩하지 않는다).
    fn replay(paths: &Paths, offline: bool) -> Result<(Derived, Vec<String>), String> {
        core_rebuild::replay(paths, offline).map_err(|e| e.to_string())
    }

    /// 캐시에 없는 텍스트를 임베딩해 채운다. **`rebuild`에서만 부른다.**
    ///
    /// `ingest`와 `context`는 §12.7의 공간에 연결까지 하고(`get_and_link`),
    /// `reading.prose`는 텍스트 캐시까지만 채운다(`get`) — 응답문 벡터가 `obs_vec`에
    /// 들어가면 군집이 "무엇을 좋아하는가"에서 흘러나간다 (§18-3 · T49).
    async fn backfill(app: &soul_pipeline::App, missing: &[String]) -> Result<usize, String> {
        let wanted: HashSet<&str> = missing.iter().map(String::as_str).collect();
        let client = app.openai().map_err(|e| e.to_string())?;
        let embedder = soul_net::embed::Embedder {
            db: &app.db,
            client: Some(&client),
            provider: core_rebuild::EMBED_PROVIDER.to_string(),
            model: app.config.embed.model.clone(),
            dims: app.config.embed.dims,
            offline: false,
        };

        let set = app.store.load_set().map_err(|e| e.to_string())?;
        let dead = set.superseded_ids();
        let mut done = 0usize;
        // ULID 순으로 훑는다 — 같은 텍스트가 여러 관측에 있어도 첫 호출 뒤에는 캐시 히트다.
        for o in set.as_slice() {
            match o {
                Observation::Ingest(i) if !dead.contains(&i.id) => {
                    if wanted.contains(i.machine.prose.as_str()) {
                        embedder
                            .get_and_link(Space::Object, i.id.as_str(), &i.machine.prose)
                            .await
                            .map_err(|e| e.to_string())?;
                        done += 1;
                    }
                }
                Observation::Context(c) => {
                    if wanted.contains(c.critique.as_str()) {
                        embedder
                            .get_and_link(Space::Critique, c.id.as_str(), &c.critique)
                            .await
                            .map_err(|e| e.to_string())?;
                        done += 1;
                    }
                }
                Observation::Reading(r) => {
                    if let Some(p) = r.prose.as_deref() {
                        if wanted.contains(p) {
                            embedder.get(p).await.map_err(|e| e.to_string())?;
                            done += 1;
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(done)
    }

    /// 관측이 하나도 없으면 `T_ref`가 없다. §R10대로 `—` 하나를 적는다.
    fn t_ref_text(derived: &Derived) -> String {
        derived
            .t_ref
            .map(|t| t.to_rfc3339_millis())
            .unwrap_or_else(|| NULL_GLYPH.to_string())
    }
}

/// 설정·진단 커맨드 중 **앱 상태 없이 성립하는 것**만 여기서 본다.
/// 나머지는 `AppState`가 실제 파일 시스템을 잡으므로 통합 경로에서 확인한다.
#[cfg(test)]
mod settings_tests {
    use super::*;

    /// §2 — 계정 이름이 바뀌면 기존 사용자의 키가 사라진 것처럼 보인다.
    /// 순서까지 프런트(`Setup.tsx: KNOWN_ACCOUNTS`)와 같아야 목록이 어긋나지 않는다.
    #[test]
    fn secret_accounts_mirror_the_frontend_list() {
        assert_eq!(
            SECRET_ACCOUNTS,
            ["openai_api_key", "search_api_key", "youtube_api_key"]
        );
    }

    /// 알 수 없는 계정은 **키체인을 건드리기 전에** 거절한다 (CI에서 안전하다).
    #[tokio::test]
    async fn set_secret_rejects_unknown_accounts() {
        let e = set_secret("아무거나".into(), "sk-비밀값".into())
            .await
            .unwrap_err();
        assert!(e.contains("알 수 없는 키 계정"), "{e}");
        // §2 — 오류 문구에 값이 들어가면 안 된다.
        assert!(!e.contains("sk-"), "{e}");
    }

    /// §D7 — 고지를 끈 사용자에게 다시 띄우지 않는다. 확인 여부만 보면 그 설정이 무시된다.
    #[test]
    fn boundary_notice_respects_both_switches() {
        let mut cfg = soul_core::config::Config::default();
        // 기본값은 "켬 + 미확인" = 보여준다 (§9.8 · §D7).
        assert!(needs_boundary_notice(&cfg));

        cfg.privacy.boundary_acknowledged = true;
        assert!(!needs_boundary_notice(&cfg));

        cfg.privacy.boundary_acknowledged = false;
        cfg.privacy.show_boundary_on_first_run = false;
        assert!(!needs_boundary_notice(&cfg));
    }

    /// T38 · §19.7 — `soul mcp --print-config`가 내는 것과 **같은 형태**다.
    /// 여기가 갈라지면 사용자가 붙여 넣은 설정이 CLI와 다른 서버를 가리킨다.
    #[tokio::test]
    async fn mcp_config_matches_the_cli_shape() {
        let s = mcp_config_json().await.unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).expect("유효한 JSON이어야 한다");
        assert_eq!(v["mcpServers"]["soul"]["command"], "soul");
        assert_eq!(v["mcpServers"]["soul"]["args"], serde_json::json!(["mcp"]));
        // 서버 목록에 우리 것 하나뿐 — 다른 클라이언트 설정을 흉내 내지 않는다.
        assert_eq!(v["mcpServers"].as_object().map(|o| o.len()), Some(1));
    }
}

// ──────────────────────────── 성찰·대시보드·아카이브 내부 (§13 화면 4·5·6)

/// 화면 3~6 커맨드가 쓰는 **읽기 전용 조회 층**.
///
/// 판단은 전부 `soul-core`에 있다. 여기서 하는 일은 그 정의를 그대로 쓰되
/// **한 번의 인덱스 패스**로 묶어 5,000건에서도 예산 안에 답하는 것뿐이다 (T39·T67).
/// 어떤 함수도 네트워크를 쓰지 않는다 (T33·T68).
mod archive_support {
    use super::{
        ArchiveItem, ArchiveQuery, CulturalCard, ItemDetail, ProposalView, ReadingView, SourceLink,
    };
    use soul_core::db::embed_cache::Space;
    use soul_core::db::queue::QueueState;
    use soul_core::derived::{pca, Cell};
    use soul_core::ids::ObsId;
    use soul_core::obs::{Axis, ContextObs, Ingest, Kind, Layer, ObsSet, Reading, Verdict};
    use soul_core::paths::Paths;
    use soul_core::vecmath;
    use std::collections::HashMap;

    /// §13 화면 6 — 이 수를 넘으면 프런트가 단색 점을 그린다.
    /// 그때 썸네일을 읽어 봐야 화면에 쓰이지 않고 예산만 넘긴다 (T67).
    /// `src/lib/types.ts`의 `TILE_RENDER_LIMIT`와 같은 값이어야 한다.
    const TILE_RENDER_LIMIT: usize = 200;

    /// §13 화면 6 산점도의 기본 축 조합.
    const DEFAULT_X_AXIS: Axis = Axis::Grain;
    const DEFAULT_Y_AXIS: Axis = Axis::Valence;

    /// `ArchiveQuery.cells`의 다섯 번째 값. `Cell`에는 없다 —
    /// 두 층 중 한쪽이 미응답이거나 문화 글귀가 없어 셀이 `null`인 항목이다 (§12.6).
    const CELL_INCOMPLETE: &str = "incomplete";

    /// §6.2 — `Quality`에는 `parse`가 없다. 패싯 값은 이 셋뿐이다.
    const QUALITY_NAMES: [&str; 3] = ["full", "partial", "minimal"];

    /// CPU·디스크 작업을 워커 스레드로 옮긴다.
    ///
    /// 커맨드는 async 런타임 위에서 돈다. 5,000건 스캔이 그 자리에서 돌면 같은 런타임의
    /// 다른 작업(투입 진행·비평 워커)이 그동안 멈춘다. 런타임이 단일 스레드면
    /// `block_in_place`가 패닉하므로 그때는 제자리에서 부른다.
    pub(super) fn blocking<T>(f: impl FnOnce() -> T) -> T {
        match tokio::runtime::Handle::try_current().map(|h| h.runtime_flavor()) {
            Ok(tokio::runtime::RuntimeFlavor::MultiThread) => tokio::task::block_in_place(f),
            _ => f(),
        }
    }

    /// 화면 4의 좌우 diff. 왼쪽은 지금의 `SOUL.md` 전문, 오른쪽은 `SOUL.next.md` 전문이다.
    ///
    /// **표시용 전문과 편집용 본문을 따로 보낸다.** 편집 상자가 전문을 물면 사용자가 고친
    /// `soul:human`이 승인 경로로 돌아와 `soul_delta`에 실리고, 그 뒤 성찰 호출마다
    /// 원격으로 나간다 (§D4·§18-4·T29). 두 값을 한 필드로 합치지 말 것.
    ///
    /// 축 변화·근거·이유는 제안 관측(`soul_delta`)에 있는 값을 그대로 옮긴다.
    /// 프런트는 계산하지 않는다 (§2).
    pub(super) fn proposal_view(p: &soul_pipeline::reflect_flow::Proposal) -> ProposalView {
        ProposalView {
            current_text: p.current_md.clone(),
            proposed_text: p.next_md.clone(),
            current_profile_text: p.current_profile_text.clone(),
            proposed_profile_text: p.proposed_profile_text.clone(),
            axis_delta: p.delta.axis_delta.clone(),
            cites: p.delta.cites.iter().map(|c| c.to_string()).collect(),
            rationale: p.delta.rationale.clone(),
        }
    }

    // ─────────────────────────────────────────────── §12.6의 2단 조인 색인

    /// `divergence::cell_of`의 정의를 **한 번의 패스**로 옮긴 색인.
    ///
    /// 정의는 그 함수와 같다 (아래 `cell_index_agrees_with_cell_of`가 그것을 고정한다).
    /// 항목마다 `cell_of`를 부르면 그 함수가 매번 관측 전체를 훑으므로 5,000건에서
    /// 아카이브 첫 렌더 예산을 넘긴다 (T67).
    struct CellIndex<'a> {
        /// ingest ID → 최신 `sensory` 응답.
        sensory: HashMap<&'a str, &'a Reading>,
        /// ingest ID → 최신 `context`. 재요청 시 최신 것만 본다 (T55b).
        context: HashMap<&'a str, &'a ContextObs>,
        /// **context ID** → 최신 `cultural` 응답. 문화 응답은 ingest를 가리키지 않는다 (T55c).
        cultural: HashMap<&'a str, &'a Reading>,
    }

    impl<'a> CellIndex<'a> {
        fn build(set: &'a ObsSet) -> CellIndex<'a> {
            let mut sensory: HashMap<&'a str, &'a Reading> = HashMap::new();
            let mut cultural: HashMap<&'a str, &'a Reading> = HashMap::new();
            let mut context: HashMap<&'a str, &'a ContextObs> = HashMap::new();
            for r in set.readings() {
                // "최신"은 언제나 ULID 최대다 — `latest_reading_for`와 같은 규칙이다.
                let slot = match r.layer {
                    Layer::Sensory => &mut sensory,
                    Layer::Cultural => &mut cultural,
                };
                slot.entry(r.target.as_str())
                    .and_modify(|cur| {
                        if r.id > cur.id {
                            *cur = r;
                        }
                    })
                    .or_insert(r);
            }
            for c in set.contexts() {
                context
                    .entry(c.target.as_str())
                    .and_modify(|cur| {
                        if c.id > cur.id {
                            *cur = c;
                        }
                    })
                    .or_insert(c);
            }
            CellIndex {
                sensory,
                context,
                cultural,
            }
        }

        fn latest_sensory(&self, ingest: &ObsId) -> Option<&'a Reading> {
            self.sensory.get(ingest.as_str()).copied()
        }

        fn latest_context(&self, ingest: &ObsId) -> Option<&'a ContextObs> {
            self.context.get(ingest.as_str()).copied()
        }

        fn latest_cultural(&self, context: &ObsId) -> Option<&'a Reading> {
            self.cultural.get(context.as_str()).copied()
        }

        /// 두 층 응답이 **모두** 있어야 셀이 정해진다. 하나라도 없으면 미완성이다 (T55).
        fn cell(&self, ingest: &ObsId) -> Option<Cell> {
            let s = self.latest_sensory(ingest)?;
            let ctx = self.latest_context(ingest)?;
            let c = self.latest_cultural(&ctx.id)?;
            Some(match (s.verdict, c.verdict) {
                (Verdict::Yes, Verdict::Yes) => Cell::Read,
                (Verdict::Yes, Verdict::No) => Cell::OtherReason,
                (Verdict::No, Verdict::Yes) => Cell::WrongWords,
                (Verdict::No, Verdict::No) => Cell::Unread,
            })
        }
    }

    // ─────────────────────────────────────────────── 질의 한 번의 스냅샷

    /// PCA 좌표를 어디까지 구할 것인가.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Pca {
        /// 캐시가 없거나 어긋나면 투영해서 캐시에 넣는다 (산점도의 구조 보기).
        Fresh,
        /// 캐시에 있는 것만 쓴다. 상세·이웃 목록은 좌표를 그리지 않는다.
        Cached,
    }

    /// 아카이브 질의 한 번이 보는 것 전부. **읽기만 한다.**
    struct Index<'a> {
        set: &'a ObsSet,
        paths: &'a Paths,
        cells: CellIndex<'a>,
        /// §R9 — supersede된 ingest는 여기 없다. 화면 6은 이것만 그린다 (T70b).
        active: Vec<&'a Ingest>,
        /// 주 공간(`obs_vec`)의 벡터. 비평 벡터를 섞지 않는다 (§12.7, T49).
        vectors: HashMap<String, Vec<f32>>,
        cluster_of: HashMap<&'a str, usize>,
        pca_of: HashMap<&'a str, (f64, f64)>,
    }

    impl<'a> Index<'a> {
        fn build(
            app: &'a soul_pipeline::App,
            set: &'a ObsSet,
            mode: Pca,
        ) -> Result<Index<'a>, String> {
            let active = set.active_ingests();
            let cells = CellIndex::build(set);
            let vectors: HashMap<String, Vec<f32>> = app
                .db
                .obs_vec_all(Space::Object)
                .map_err(|e| e.to_string())?
                .into_iter()
                .collect();

            // 군집·PCA의 입력은 **벡터가 있는 활성 ingest를 ULID 오름차순으로 늘어놓은 것**이다.
            // `soul-pipeline::ingest::clustering_for`가 캐시를 만들 때 쓴 순서와 같아야
            // 배정이 항목에 맞게 붙는다 (§R5의 입력 순서).
            let embedded: Vec<&'a Ingest> = active
                .iter()
                .copied()
                .filter(|i| vectors.contains_key(i.id.as_str()))
                .collect();

            Ok(Index {
                cluster_of: cluster_assignment(app, &embedded, &vectors)?,
                pca_of: pca_coords(app, set, &embedded, &vectors, mode)?,
                set,
                paths: &app.paths,
                cells,
                active,
                vectors,
            })
        }

        /// 화면 6의 타일 하나. `x`/`y`는 **Rust가 이미 계산해서 주는 값**이다.
        fn item(&self, i: &Ingest, x: Axis, y: Axis, thumb: bool) -> ArchiveItem {
            let coords = self.pca_of.get(i.id.as_str()).copied();
            ArchiveItem {
                id: i.id.to_string(),
                kind: i.source.kind.as_str().to_string(),
                thumb_data_url: if thumb { self.thumb(i) } else { None },
                prose: i.machine.prose.clone(),
                tags: i.machine.tags.clone(),
                surprisal: i.surprisal,
                quality: i.machine.quality.as_str().to_string(),
                cell: self.cells.cell(&i.id).map(|c| c.as_str().to_string()),
                cluster: self.cluster_of.get(i.id.as_str()).copied(),
                x: i.machine.axes.get(x),
                y: i.machine.axes.get(y),
                px: coords.map(|c| c.0),
                py: coords.map(|c| c.1),
                month: i.ts.month().to_string(),
            }
        }

        /// §20.4 — 썸네일 파일 키는 `source.sha256`이다. 없으면 `None`이고
        /// 프런트가 서술문 앞 40자를 그린다 (T70c).
        fn thumb(&self, i: &Ingest) -> Option<String> {
            super::pipeline_support::thumb_data_url(&self.paths.thumb_file(&i.source.sha256))
        }

        /// 화면 6이 고를 수 있는 것은 **살아 있는 투입**뿐이다. 아니면 사유를 말한다 (§R9).
        fn active_ingest(&self, id: &ObsId) -> Result<&'a Ingest, String> {
            if let Some(i) = self.active.iter().copied().find(|i| &i.id == id) {
                return Ok(i);
            }
            match self.set.get(id) {
                None => Err(format!("관측 {id} 을 찾을 수 없습니다")),
                Some(o) if o.as_ingest().is_some() => Err(format!(
                    "관측 {id} 는 재분석으로 대체되었습니다 (§R9). 대체본을 여십시오"
                )),
                Some(o) => Err(format!(
                    "관측 {id} 는 투입(ingest)이 아니라 {} 입니다",
                    o.type_name()
                )),
            }
        }
    }

    /// §12.3 — **캐시된 배정을 읽는다.** 여기서 k-means를 다시 돌리지 않는다.
    ///
    /// 캐시가 만들어진 뒤 투입이 늘면 배정 길이가 항목 수와 어긋난다. 재군집 시점은
    /// 투입 경로가 `should_recluster`로 정하므로(T46), 그 사이에는 **캐시된 중심에 가장
    /// 가까운 군집**으로 표시만 한다. 화면의 색이 잠시 근사가 되는 편이, 아카이브를 열
    /// 때마다 k-means가 도는 것보다 낫다.
    fn cluster_assignment<'a>(
        app: &soul_pipeline::App,
        embedded: &[&'a Ingest],
        vectors: &HashMap<String, Vec<f32>>,
    ) -> Result<HashMap<&'a str, usize>, String> {
        let mut out: HashMap<&'a str, usize> = HashMap::new();
        // 4건 미만이면 군집 자체가 없다 (§12.3). 값이 비어 있는 것이 정직하다 (§R10).
        let Some((_, clustering)) = app.db.cluster_get().map_err(|e| e.to_string())? else {
            return Ok(out);
        };
        if clustering.assignment.len() == embedded.len() {
            for (ing, k) in embedded.iter().zip(clustering.assignment.iter()) {
                out.insert(ing.id.as_str(), *k);
            }
            return Ok(out);
        }
        for ing in embedded {
            let Some(v) = vectors.get(ing.id.as_str()) else {
                continue;
            };
            let mut best: Option<(usize, f32)> = None;
            for (k, c) in clustering.centroids.iter().enumerate() {
                let s = vecmath::cosine_similarity(v, c);
                if best.is_none_or(|(_, top)| s > top) {
                    best = Some((k, s));
                }
            }
            if let Some((k, _)) = best {
                out.insert(ing.id.as_str(), k);
            }
        }
        Ok(out)
    }

    /// §13 화면 6 — 구조 보기 좌표. **`T_ref`의 날짜가 바뀔 때만** 무효화한다.
    ///
    /// 같은 날 안에서 투입이 늘면 캐시된 좌표 수가 점의 수와 어긋난다. 캐시 키에는
    /// 날짜밖에 없어 어느 좌표가 어느 항목의 것인지 알 방법이 없으므로, 그때는 다시
    /// 투영한다 — 좌표를 엉뚱한 항목에 붙이는 것보다 낫다.
    fn pca_coords<'a>(
        app: &soul_pipeline::App,
        set: &ObsSet,
        embedded: &[&'a Ingest],
        vectors: &HashMap<String, Vec<f32>>,
        mode: Pca,
    ) -> Result<HashMap<&'a str, (f64, f64)>, String> {
        let mut out: HashMap<&'a str, (f64, f64)> = HashMap::new();
        // §R1 — 기준점은 벽시계가 아니라 `T_ref`다. 관측이 없으면 좌표도 없다.
        let Some(t_ref) = set.t_ref() else {
            return Ok(out);
        };
        let date = t_ref.date_string();
        let cached = app
            .db
            .pca_get(&date)
            .map_err(|e| e.to_string())?
            .filter(|c| c.len() == embedded.len());

        let coords = match cached {
            Some(c) => c,
            None if mode == Pca::Cached => return Ok(out),
            None => {
                let vs: Vec<Vec<f32>> = embedded
                    .iter()
                    .filter_map(|i| vectors.get(i.id.as_str()).cloned())
                    .collect();
                // 결정론적이다 — 고정 시드가 필요 없고 2회 실행 시 좌표가 같다 (T69).
                let c = pca::project2(&vs);
                app.db.pca_put(&date, &c).map_err(|e| e.to_string())?;
                c
            }
        };
        for (ing, p) in embedded.iter().zip(coords) {
            out.insert(ing.id.as_str(), p);
        }
        Ok(out)
    }

    // ───────────────────────────────────────────────────────── 커맨드 본체

    /// §13 화면 6 — 패싯·검색은 전부 로컬이다. **API 호출 0건** (T68).
    pub(super) fn run_query(
        app: &soul_pipeline::App,
        q: &ArchiveQuery,
    ) -> Result<Vec<ArchiveItem>, String> {
        let set = app.store.load_set().map_err(|e| e.to_string())?;
        let ix = Index::build(app, &set, Pca::Fresh)?;

        let x = axis_of(q.x_axis.as_deref(), DEFAULT_X_AXIS)?;
        let y = axis_of(q.y_axis.as_deref(), DEFAULT_Y_AXIS)?;
        let kinds = kinds_of(&q.kinds)?;
        let qualities = qualities_of(&q.qualities)?;
        let (cells, incomplete) = cells_of(&q.cells)?;
        let needle = q
            .search
            .as_deref()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty());
        // 검색이 없으면 말뭉치를 만들지 않는다 — 패싯만 만지는 흔한 경우가 공짜여야 한다.
        let corpus = needle.as_ref().map(|_| search_corpus(&set));

        let mut hits: Vec<&Ingest> = Vec::new();
        for i in ix.active.iter().copied() {
            if !kinds.is_empty() && !kinds.contains(&i.source.kind) {
                continue;
            }
            if !qualities.is_empty() && !qualities.contains(&i.machine.quality.as_str()) {
                continue;
            }
            if q.surprisal_min.is_some_and(|lo| i.surprisal < lo) {
                continue;
            }
            if q.surprisal_max.is_some_and(|hi| i.surprisal > hi) {
                continue;
            }
            if !q.months.is_empty() {
                let m = i.ts.month().to_string();
                if !q.months.contains(&m) {
                    continue;
                }
            }
            // 한 패싯 안의 여러 값은 **OR**다 (kind·quality·기간·셀과 같은 규칙).
            // AND로 읽으면 태그 두 개를 고르는 순간 대개 0건이 되어 패싯이 쓸모없어진다.
            if !q.tags.is_empty() && !q.tags.iter().any(|t| i.machine.tags.iter().any(|x| x == t)) {
                continue;
            }
            if !q.cells.is_empty() {
                match ix.cells.cell(&i.id) {
                    Some(c) if cells.contains(&c) => {}
                    None if incomplete => {}
                    _ => continue,
                }
            }
            if q.cluster.is_some() && ix.cluster_of.get(i.id.as_str()).copied() != q.cluster {
                continue;
            }
            if let (Some(n), Some(corpus)) = (needle.as_deref(), corpus.as_ref()) {
                if !corpus.get(i.id.as_str()).is_some_and(|hay| hay.contains(n)) {
                    continue;
                }
            }
            hits.push(i);
        }

        // T67 — 200건을 넘으면 프런트가 점을 찍는다. 그때 썸네일을 읽으면 예산만 넘긴다.
        let thumbs = hits.len() <= TILE_RENDER_LIMIT;
        Ok(hits.into_iter().map(|i| ix.item(i, x, y, thumbs)).collect())
    }

    /// §19.5 `soul_similar`과 같은 연산 — **저장된 벡터끼리** 코사인 (T33).
    pub(super) fn run_neighbors(
        app: &soul_pipeline::App,
        id: &str,
        n: usize,
    ) -> Result<Vec<ArchiveItem>, String> {
        let set = app.store.load_set().map_err(|e| e.to_string())?;
        // 이웃 목록은 좌표를 그리지 않는다. 여기서 PCA를 새로 돌리지 않는다.
        let ix = Index::build(app, &set, Pca::Cached)?;
        let id = ObsId::parse(id).map_err(|e| e.to_string())?;
        ix.active_ingest(&id)?;

        let target = ix.vectors.get(id.as_str()).ok_or_else(|| {
            // 여기서 임베딩을 만들지 않는다 (T68). 사유와 조치를 말한다 (T36과 같은 규칙).
            format!("관측 {id} 의 임베딩이 캐시에 없습니다. `soul rebuild`로 캐시를 채우십시오")
        })?;

        let mut scored: Vec<(f32, &Ingest)> = ix
            .active
            .iter()
            .copied()
            .filter(|i| i.id != id)
            // 활성 ingest가 아닌 벡터(재분석으로 대체된 것 등)는 후보에 없다 (§R9).
            .filter_map(|i| {
                ix.vectors
                    .get(i.id.as_str())
                    .map(|v| (vecmath::cosine_similarity(target, v), i))
            })
            .collect();
        // 동점이면 ULID 오름차순 — 순서가 실행마다 흔들리면 안 된다.
        scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
        scored.truncate(n);

        let thumbs = scored.len() <= TILE_RENDER_LIMIT;
        Ok(scored
            .into_iter()
            .map(|(_, i)| ix.item(i, DEFAULT_X_AXIS, DEFAULT_Y_AXIS, thumbs))
            .collect())
    }

    /// §13 화면 6 항목 상세 — 두 글귀를 나란히 놓는 데 필요한 것 전부.
    pub(super) fn run_detail(app: &soul_pipeline::App, id: &str) -> Result<ItemDetail, String> {
        let set = app.store.load_set().map_err(|e| e.to_string())?;
        let ix = Index::build(app, &set, Pca::Cached)?;
        let id = ObsId::parse(id).map_err(|e| e.to_string())?;
        let ingest = ix.active_ingest(&id)?;

        // §12.6 — 재요청된 문화 글귀는 최신 것만 보여준다. 이전 것에 달린 응답이
        // 셀 계산에서 빠지는 것과 같은 이유로, 화면에도 최신 것만 오른다 (T55b).
        let context = ix.cells.latest_context(&id);
        let cultural_reading = context
            .and_then(|c| ix.cells.latest_cultural(&c.id))
            .map(reading_view);

        Ok(ItemDetail {
            item: ix.item(ingest, DEFAULT_X_AXIS, DEFAULT_Y_AXIS, true),
            origin: ingest.source.origin.clone(),
            sensory_prose: ingest.machine.prose.clone(),
            // `None`이면 **아직 답하지 않은 층**이다. 프런트가 그 자리에서 ○/×를 받는다 (T70).
            sensory_reading: ix.cells.latest_sensory(&id).map(reading_view),
            context: context.map(|c| cultural_card(&ix, ingest, c)),
            cultural_reading,
            // §9.10 — 큐에서 3회까지 실패한 뒤 남은 상태. 그때 재시도 버튼의 뜻이 분명하다.
            context_failed: context.is_none() && failed_in_queue(app, id.as_str())?,
            // §9.3 — 뒤집기는 YouTube 항목의 audio ↔ video 추정을 되돌리는 동작이다.
            //         다른 경로에는 추정 자체가 없다.
            can_recast: soul_media::probe::youtube_video_id(&ingest.source.origin).is_some()
                && matches!(ingest.source.kind, Kind::Audio | Kind::Video),
        })
    }

    // ───────────────────────────────────────────────────────────── 도우미

    fn reading_view(r: &Reading) -> ReadingView {
        ReadingView {
            verdict: r.verdict.as_str().to_string(),
            // `verdict = yes`면 언제나 `null`이다 (§6.3, T7).
            prose: r.prose.clone(),
            divergence: r.divergence,
        }
    }

    fn cultural_card(ix: &Index<'_>, ingest: &Ingest, c: &ContextObs) -> CulturalCard {
        CulturalCard {
            context_id: c.id.to_string(),
            ingest_id: ingest.id.to_string(),
            critique: c.critique.clone(),
            lineage: c.lineage.clone(),
            // §6.4 — 파이프라인이 센 값이다. 여기서 다시 세지 않는다 (T58).
            grounded: c.grounded,
            // 실제 검색어(`queries`)는 내보내지 않는다 — 프라이버시 고지의 대상이다 (§D6).
            sources: c
                .sources
                .iter()
                .map(|s| SourceLink {
                    url: s.url.clone(),
                    title: s.title.clone(),
                })
                .collect(),
            thumb_data_url: ix.thumb(ingest),
        }
    }

    /// §9.10 — 문화 글귀가 없는 이유가 "실패"인지 "아직"인지 가른다.
    fn failed_in_queue(app: &soul_pipeline::App, ingest_id: &str) -> Result<bool, String> {
        Ok(app
            .db
            .queue_items(Some(QueueState::Failed))
            .map_err(|e| e.to_string())?
            .iter()
            .any(|q| q.ingest_id == ingest_id))
    }

    /// 검색 대상 (§13 화면 6): `machine.prose` · `context.critique` · `reading.prose` · `tags`.
    ///
    /// **부분 문자열 일치만.** 의미 검색은 질의를 임베딩해야 하므로 넣지 않는다
    /// (§19.5와 같은 이유, T68). 대소문자는 무시한다 — 한글에는 영향이 없고,
    /// 라틴 문자에서 `Shoegaze`와 `shoegaze`가 다른 결과를 주는 것은 설명할 수 없는 차이다.
    fn search_corpus(set: &ObsSet) -> HashMap<String, String> {
        let mut out: HashMap<String, String> = HashMap::new();
        for i in set.active_ingests() {
            let mut hay = i.machine.prose.to_lowercase();
            for t in &i.machine.tags {
                hay.push('\n');
                hay.push_str(&t.to_lowercase());
            }
            out.insert(i.id.to_string(), hay);
        }
        // 문화 응답은 ingest가 아니라 context를 가리킨다. 되짚어야 항목에 붙는다 (T55c).
        let mut ingest_of_context: HashMap<&str, &ObsId> = HashMap::new();
        for c in set.contexts() {
            ingest_of_context.insert(c.id.as_str(), &c.target);
            if let Some(hay) = out.get_mut(c.target.as_str()) {
                hay.push('\n');
                hay.push_str(&c.critique.to_lowercase());
            }
        }
        for r in set.readings() {
            let Some(prose) = r.prose.as_deref() else {
                continue;
            };
            let target = match r.layer {
                Layer::Sensory => Some(r.target.as_str()),
                Layer::Cultural => ingest_of_context.get(r.target.as_str()).map(|i| i.as_str()),
            };
            if let Some(hay) = target.and_then(|t| out.get_mut(t)) {
                hay.push('\n');
                hay.push_str(&prose.to_lowercase());
            }
        }
        out
    }

    fn axis_of(name: Option<&str>, default: Axis) -> Result<Axis, String> {
        match name.map(str::trim).filter(|s| !s.is_empty()) {
            None => Ok(default),
            Some(s) => Axis::parse(s).ok_or_else(|| {
                format!(
                    "알 수 없는 축: {s} (가능: {})",
                    soul_core::obs::AXIS_NAMES.join(", ")
                )
            }),
        }
    }

    fn kinds_of(v: &[String]) -> Result<Vec<Kind>, String> {
        v.iter()
            .map(|s| {
                Kind::parse(s)
                    .ok_or_else(|| format!("알 수 없는 kind: {s} (text|image|audio|video)"))
            })
            .collect()
    }

    fn qualities_of(v: &[String]) -> Result<Vec<&'static str>, String> {
        v.iter()
            .map(|s| {
                QUALITY_NAMES
                    .into_iter()
                    .find(|q| *q == s.as_str())
                    .ok_or_else(|| format!("알 수 없는 quality: {s} (full|partial|minimal)"))
            })
            .collect()
    }

    /// 다섯 번째 패싯 값 `incomplete`는 `Cell::parse`가 모른다 — 셀이 `null`인 항목이므로
    /// 여기서 따로 받는다 (§13 화면 6의 "미완성").
    fn cells_of(v: &[String]) -> Result<(Vec<Cell>, bool), String> {
        let mut cells = Vec::new();
        let mut incomplete = false;
        for s in v {
            if s == CELL_INCOMPLETE {
                incomplete = true;
                continue;
            }
            match Cell::parse(s) {
                Some(c) => cells.push(c),
                None => {
                    return Err(format!(
                    "알 수 없는 셀: {s} (read|other_reason|wrong_words|unread|{CELL_INCOMPLETE})"
                ))
                }
            }
        }
        Ok((cells, incomplete))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use soul_core::obs::{
            new_header, Axes, Machine, ModelRef, Observation, Quality, Source, SourceRef,
        };
        use soul_core::time::Ts;

        fn model() -> ModelRef {
            ModelRef {
                provider: "test".into(),
                id: "m".into(),
                prompt_sha256: None,
                calls: vec![],
            }
        }

        fn ingest(prose: &str, tags: &[&str]) -> Observation {
            let (id, ts, schema) = new_header();
            Observation::Ingest(Ingest {
                id,
                ts,
                schema,
                source: Source {
                    kind: Kind::Image,
                    sha256: "ab".repeat(32),
                    origin: "file:///x.jpg".into(),
                    bytes: 1,
                    mime: "image/jpeg".into(),
                },
                machine: Machine {
                    prose: prose.into(),
                    axes: Axes::from_array([0.5; 8]),
                    tags: tags.iter().map(|t| (*t).to_string()).collect(),
                    quality: Quality::Full,
                    prompt_sha256: "p".into(),
                },
                min_dist: None,
                surprisal: 0.5,
                model: model(),
                supersedes: None,
            })
        }

        fn context(target: &ObsId, critique: &str) -> Observation {
            let (id, ts, schema) = new_header();
            let src = |u: &str| SourceRef {
                url: u.into(),
                title: u.into(),
                fetched_at: ts,
            };
            Observation::Context(ContextObs {
                id,
                ts,
                schema,
                target: target.clone(),
                critique: critique.into(),
                lineage: vec!["슈게이즈".into()],
                queries: vec!["q".into()],
                sources: vec![src("https://a.example"), src("https://b.example")],
                grounded: true,
                model: model(),
            })
        }

        fn reading(
            layer: Layer,
            target: &ObsId,
            verdict: Verdict,
            prose: Option<&str>,
        ) -> Observation {
            let (id, ts, schema) = new_header();
            Observation::Reading(Reading {
                id,
                ts,
                schema,
                layer,
                target: target.clone(),
                verdict,
                prose: prose.map(str::to_string),
                divergence: prose.map(|_| 0.4),
            })
        }

        /// `CellIndex`는 `divergence::cell_of`를 한 번의 패스로 옮긴 것이다.
        /// 정의가 갈리면 화면 6의 색과 대시보드의 2×2 개수가 서로 다른 말을 한다 (§12.6).
        #[test]
        fn cell_index_agrees_with_cell_of() {
            let mut obs: Vec<Observation> = Vec::new();
            let mut targets: Vec<ObsId> = Vec::new();

            // 네 조합 + 한 층만 답한 경우들.
            let combos = [
                (Some(Verdict::Yes), Some(Verdict::Yes)),
                (Some(Verdict::Yes), Some(Verdict::No)),
                (Some(Verdict::No), Some(Verdict::Yes)),
                (Some(Verdict::No), Some(Verdict::No)),
                (Some(Verdict::Yes), None),
                (None, Some(Verdict::Yes)),
                (None, None),
            ];
            for (s, c) in combos {
                let i = ingest("차갑고 정돈된 실내", &["실내"]);
                let iid = i.id().clone();
                obs.push(i);
                if let Some(v) = s {
                    obs.push(reading(Layer::Sensory, &iid, v, None));
                }
                let ctx = context(&iid, "느와르 도시 이미지의 관습을 따른다");
                let cid = ctx.id().clone();
                obs.push(ctx);
                if let Some(v) = c {
                    obs.push(reading(Layer::Cultural, &cid, v, None));
                }
                targets.push(iid);
            }

            // T55b — 재요청된 context. **이전 context에 달린 응답은 셀에서 무시된다.**
            let i = ingest("두 번 비평한 항목", &[]);
            let iid = i.id().clone();
            obs.push(i);
            obs.push(reading(Layer::Sensory, &iid, Verdict::Yes, None));
            let old = context(&iid, "첫 비평");
            let old_id = old.id().clone();
            obs.push(old);
            obs.push(reading(Layer::Cultural, &old_id, Verdict::Yes, None));
            obs.push(context(&iid, "다시 만든 비평"));
            targets.push(iid);

            // context가 아예 없는 항목 (비평 대기 중이거나 실패).
            let i = ingest("비평 없음", &[]);
            let iid = i.id().clone();
            obs.push(i);
            obs.push(reading(Layer::Sensory, &iid, Verdict::No, None));
            targets.push(iid);

            let set = ObsSet::new(obs);
            let ix = CellIndex::build(&set);
            let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
            for id in &targets {
                let want = soul_core::derived::divergence::cell_of(&set, id);
                assert_eq!(ix.cell(id), want, "{id}");
                if let Some(c) = want {
                    seen.insert(c.as_str());
                }
            }
            assert_eq!(
                seen.len(),
                4,
                "네 셀이 모두 나와야 비교가 공허하지 않다: {seen:?}"
            );
            assert!(
                targets.iter().any(|id| ix.cell(id).is_none()),
                "미완성 항목도 있어야 한다"
            );
        }

        /// 검색은 네 곳을 본다 (§13 화면 6). 문화 층은 2단 조인을 되짚어야 항목에 붙는다.
        #[test]
        fn search_corpus_covers_prose_tags_critique_and_corrections() {
            let i = ingest("차갑고 정돈된 실내", &["실내", "무인"]);
            let iid = i.id().clone();
            let ctx = context(&iid, "Shoegaze 계보 위에 있다");
            let cid = ctx.id().clone();
            let set = ObsSet::new(vec![
                i,
                ctx,
                reading(
                    Layer::Sensory,
                    &iid,
                    Verdict::No,
                    Some("실은 습도가 먼저다"),
                ),
                reading(
                    Layer::Cultural,
                    &cid,
                    Verdict::No,
                    Some("느와르라기보다 퇴근길"),
                ),
            ]);

            let corpus = search_corpus(&set);
            let hay = corpus.get(iid.as_str()).expect("활성 ingest는 언제나 있다");
            assert!(hay.contains("정돈된"), "machine.prose");
            assert!(hay.contains("무인"), "tags");
            assert!(hay.contains("계보"), "context.critique");
            assert!(hay.contains("습도"), "sensory reading.prose");
            assert!(hay.contains("퇴근길"), "cultural reading.prose (2단 조인)");
            assert!(hay.contains("shoegaze"), "대소문자를 무시한다");
        }

        /// supersede된 ingest는 검색에도 화면에도 나오지 않는다 (§R9, T70b).
        #[test]
        fn superseded_ingests_are_not_searchable() {
            let old = ingest("뒤집기 전 서술", &[]);
            let old_id = old.id().clone();
            let mut new = ingest("뒤집은 뒤 서술", &[]);
            if let Observation::Ingest(i) = &mut new {
                i.supersedes = Some(old_id.clone());
            }
            let new_id = new.id().clone();
            let set = ObsSet::new(vec![old, new]);

            let corpus = search_corpus(&set);
            assert!(corpus.contains_key(new_id.as_str()));
            assert!(
                !corpus.contains_key(old_id.as_str()),
                "대체된 항목이 검색에 남으면 개수가 어긋나 보인다"
            );
        }

        #[test]
        fn cells_facet_accepts_the_incomplete_value() {
            let (cells, incomplete) =
                cells_of(&["read".to_string(), CELL_INCOMPLETE.to_string()]).unwrap();
            assert_eq!(cells, vec![Cell::Read]);
            assert!(incomplete);

            let (cells, incomplete) = cells_of(&[]).unwrap();
            assert!(cells.is_empty() && !incomplete);
            assert!(
                cells_of(&["maybe".to_string()]).is_err(),
                "T59 — 중간값은 없다"
            );
        }

        #[test]
        fn axes_default_to_grain_by_valence() {
            assert_eq!(axis_of(None, DEFAULT_X_AXIS).unwrap(), Axis::Grain);
            assert_eq!(axis_of(None, DEFAULT_Y_AXIS).unwrap(), Axis::Valence);
            assert_eq!(axis_of(Some(""), DEFAULT_X_AXIS).unwrap(), Axis::Grain);
            assert_eq!(
                axis_of(Some("chroma"), DEFAULT_X_AXIS).unwrap(),
                Axis::Chroma
            );
            assert!(axis_of(Some("없는축"), DEFAULT_X_AXIS).is_err());
        }

        #[test]
        fn facet_values_are_validated() {
            assert_eq!(kinds_of(&["image".to_string()]).unwrap(), vec![Kind::Image]);
            assert!(kinds_of(&["gif".to_string()]).is_err());
            assert_eq!(
                qualities_of(&["partial".to_string()]).unwrap(),
                vec!["partial"]
            );
            assert!(qualities_of(&["좋음".to_string()]).is_err());
        }

        /// 화면 4는 제안 관측의 값을 **그대로** 옮긴다. 여기서 다시 계산하지 않는다 (§2).
        #[test]
        fn proposal_view_carries_the_delta_verbatim() {
            let (id, ts, schema) = new_header();
            let mut axis_delta = soul_core::obs::AxisDelta::new();
            axis_delta.insert("grain".to_string(), 0.05);
            let cite = soul_core::ids::new_id();
            let delta = soul_core::obs::SoulDelta {
                id: id.clone(),
                ts,
                schema,
                window: soul_core::obs::Window {
                    from: id.clone(),
                    to: id,
                },
                blocks: std::collections::BTreeMap::new(),
                axis_delta,
                morphology_delta: None,
                cites: vec![cite.clone()],
                rationale: "other_reason 셀이 늘었다".into(),
                model: model(),
            };
            let p = soul_pipeline::reflect_flow::Proposal {
                delta,
                next_md:
                    "# SOUL\n제안본\n<!-- soul:human -->\n사람이 쓴 줄\n<!-- /soul:human -->\n"
                        .into(),
                current_md: "# SOUL\n지금\n".into(),
                current_profile_text: "지금 문장.".into(),
                proposed_profile_text: "제안된 문장.".into(),
            };

            let v = proposal_view(&p);
            assert_eq!(v.current_text, "# SOUL\n지금\n");
            assert!(v.proposed_text.contains("제안본"));
            // 편집용 값은 전문이 아니라 `profile` 본문이다 (§D4·T29).
            assert_eq!(v.current_profile_text, "지금 문장.");
            assert_eq!(v.proposed_profile_text, "제안된 문장.");
            assert!(
                !v.proposed_profile_text.contains("사람이 쓴 줄"),
                "편집 상자에 `soul:human`이 실리면 안 된다"
            );
            assert_eq!(v.axis_delta.get("grain"), Some(&0.05));
            assert_eq!(v.cites, vec![cite.to_string()]);
            assert_eq!(v.rationale, "other_reason 셀이 늘었다");
        }

        /// PCA 캐시 키는 `T_ref`의 **날짜**다. 같은 날은 같은 키여야 한다 (§13 화면 6).
        #[test]
        fn pca_cache_key_is_the_t_ref_date() {
            let a = Ts::parse("2026-08-13T09:12:33.123Z").unwrap();
            let b = Ts::parse("2026-08-13T23:59:59.999Z").unwrap();
            let c = Ts::parse("2026-08-14T00:00:00.000Z").unwrap();
            assert_eq!(a.date_string(), b.date_string(), "같은 날은 같은 키다");
            assert_ne!(b.date_string(), c.date_string());
        }
    }
}
