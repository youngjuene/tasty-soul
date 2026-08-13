//! Tauri 셸 (§2 · §13).
//!
//! **프런트는 순수 뷰다.** 모든 판단·파일 I/O·API 호출은 여기 아래 Rust에 있다.
//! **API 키를 프런트엔드로 반환하는 커맨드를 만들지 않는다** (§2) —
//! 노출은 `키가 설정되어 있는가`라는 불리언까지다.
//!
//! §R7 — Tauri single-instance 플러그인을 함께 쓴다.
//! §20.1 — 앱을 닫으면 비평 워커도 함께 멈춘다. 상주 프로세스를 남기지 않는다 (T44).

pub mod commands;
pub mod state;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tauri::{Emitter as _, Manager as _};
use tauri_plugin_clipboard_manager::ClipboardExt as _;
use tauri_plugin_global_shortcut::{GlobalShortcutExt as _, ShortcutState};

/// §13 화면 1 — 클립보드 투입. `src/screens/Ingest.tsx`의 `SHORTCUT`과 **같은 문자열**이다.
const CLIPBOARD_SHORTCUT: &str = "CommandOrControl+Shift+V";

/// 셸이 단축키를 직접 처리했을 때 프런트에 알리는 이벤트.
///
/// 화면 1이 떠 있는 동안은 프런트가 자기 핸들러를 등록하므로 이 경로가 돌지 않는다
/// (§13 화면 1 — 진행 표시가 그쪽에 있어 그 편이 낫다). 다른 탭에 있을 때만 여기가 받는다.
const EVENT_CLIPBOARD_CARD: &str = "ingest://cards";
const EVENT_CLIPBOARD_ERROR: &str = "ingest://error";

/// 프런트가 화면 1을 떠나며 단축키를 해제하면 아무도 듣지 않는 상태가 된다.
/// 전역 단축키는 어느 화면에 있든 살아 있어야 하므로 이 간격으로 되찾는다.
const SHORTCUT_REARM_INTERVAL: Duration = Duration::from_millis(1500);

/// §9.10 · T51c — 지난 실행이 남긴 큐를 이어받기 전에 창이 먼저 뜨도록 물러서는 시간.
const STARTUP_DRAIN_DELAY: Duration = Duration::from_secs(3);

/// 종료 시 `AppState.app` 잠금을 기다리는 상한. 넘으면 그냥 프로세스를 끝낸다.
const SHUTDOWN_LOCK_WAIT: Duration = Duration::from_secs(2);

/// `ExitRequested`와 `Exit`가 모두 온다. 정리는 한 번이면 된다.
static SHUTDOWN_DONE: AtomicBool = AtomicBool::new(false);

pub fn run() {
    let mut app_state = state::AppState::default();

    // §3 — 최초 실행 절차. **여기서 실패해도 창은 뜬다.**
    //
    // 패닉하면 사용자는 아무 화면도 못 보고, 무엇이 잘못됐는지도 알 수 없다.
    // 사유를 `AppState.open_error`에 담아 두면 `setup_status`가 그대로 전달하고,
    // `App.tsx`의 부팅 오류 화면이 «다시 시도»를 띄운다.
    match soul_core::paths::Paths::discover() {
        Ok(paths) => {
            // `open_or_init`이 스켈레톤을 쓰기 **전에** 봐야 한다 (§3).
            app_state
                .first_run
                .store(!paths.soul_md().is_file(), Ordering::Relaxed);
            match soul_pipeline::App::open_or_init(paths) {
                Ok(app) => *app_state.app.get_mut() = Some(app),
                Err(e) => note_open_error(&mut app_state, e.to_string()),
            }
        }
        Err(e) => note_open_error(&mut app_state, e.to_string()),
    }

    let built = tauri::Builder::default()
        // §R7 — 단일 writer. 두 번째 인스턴스는 기존 창을 포커스하고 끝난다.
        // **가장 먼저 등록한다** — 두 번째 프로세스는 다른 플러그인을 세우기 전에 물러나야 한다.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            focus_main(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(app_state)
        .setup(|app| {
            arm_clipboard_shortcut(app.handle());
            spawn_shortcut_rearm(app.handle().clone());
            spawn_startup_drain(app.handle().clone());
            Ok(())
        })
        // **여기 빠진 커맨드는 프런트에서 조용히 실패한다.**
        // `src/lib/api.ts`가 부르는 것 전부가 이 목록에 있어야 한다.
        .invoke_handler(tauri::generate_handler![
            // 설정 · 진단 (§9.9 · §D7)
            commands::setup_status,
            commands::acknowledge_boundary,
            commands::set_secret,
            commands::secrets_status,
            commands::doctor,
            commands::get_config,
            commands::set_config,
            // 투입 (§13 화면 1)
            commands::ingest_files,
            commands::ingest_clipboard,
            commands::critique_pending_count,
            // ○/× 응답 (§13 화면 2 · 2.1)
            commands::record_reading,
            commands::pending_cultural_cards,
            commands::recast_kind,
            commands::recast_preview,
            commands::redo_context,
            // SOUL.md (§13 화면 3)
            commands::read_soul_md,
            commands::save_soul_md,
            // 성찰 (§13 화면 4)
            commands::reflect,
            commands::approve_proposal,
            commands::reject_proposal,
            // 대시보드 (§13 화면 5)
            commands::dashboard,
            // 아카이브 (§13 화면 6)
            commands::archive_query,
            commands::archive_neighbors,
            commands::archive_detail,
            // 기타 (§14 · §19.7)
            commands::mcp_config_json,
            commands::export_prompt,
            commands::trace_purge,
            commands::rebuild,
        ])
        .build(tauri::generate_context!());

    let app = match built {
        Ok(a) => a,
        Err(e) => {
            eprintln!("앱을 시작하지 못했습니다: {e}");
            std::process::exit(1);
        }
    };

    app.run(|handle, event| {
        // §20.1 · T44 — 닫으면 비평 워커도 함께 멈춘다. 상주 프로세스를 남기지 않는다.
        if matches!(
            event,
            tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
        ) {
            shutdown(handle);
        }
    });
}

fn note_open_error(state: &mut state::AppState, reason: String) {
    eprintln!("앱 상태를 열지 못했습니다: {reason}");
    if let Ok(slot) = state.open_error.get_mut() {
        *slot = Some(reason);
    }
}

// ─────────────────────────────────────────────────────────── §R7 single-instance

/// 두 번째 인스턴스가 왔다. 새 창을 만들지 않고 있던 창을 앞으로 가져온다.
fn focus_main(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

// ───────────────────────────────────────────── §13 화면 1 전역 단축키

/// `Cmd/Ctrl+Shift+V` — 클립보드를 읽어 `ingest_clipboard` 흐름으로 보낸다.
fn arm_clipboard_shortcut(handle: &tauri::AppHandle) {
    // 다른 앱이 이 조합을 쥐고 있으면 실패한다. 그때도 화면 1의
    // «클립보드에서 붙여넣기» 버튼이 남으므로 투입 경로가 사라지지는 않는다.
    if let Err(e) = try_arm_clipboard_shortcut(handle) {
        eprintln!("전역 단축키 {CLIPBOARD_SHORTCUT} 등록 실패: {e}");
    }
}

fn try_arm_clipboard_shortcut(handle: &tauri::AppHandle) -> Result<(), String> {
    handle
        .global_shortcut()
        .on_shortcut(CLIPBOARD_SHORTCUT, |app, _shortcut, event| {
            // 눌림만 받는다. 뗄 때 한 번 더 처리하면 같은 것이 두 번 투입된다.
            if !matches!(event.state, ShortcutState::Pressed) {
                return;
            }
            let app = app.clone();
            // 핸들러는 메인 스레드에서 불린다. 투입은 수십 초짜리다 (§9.1 표) — 넘긴다.
            tauri::async_runtime::spawn(async move { clipboard_ingest(app).await });
        })
        .map_err(|e| e.to_string())
}

/// 프런트가 단축키를 가져갔다가(화면 1 마운트) 놓으면(언마운트) 아무도 듣지 않는다.
///
/// 같은 플러그인을 쓰므로 프런트의 `register`는 이 핸들러를 **덮어쓴다** — 겹쳐 도는 일은
/// 없다. 화면 1이 살아 있는 동안은 그쪽이 진행 표시까지 하므로 그 편이 낫고,
/// 화면을 떠난 뒤에는 여기가 되찾는다.
fn spawn_shortcut_rearm(handle: tauri::AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(SHORTCUT_REARM_INTERVAL);
        match handle.try_state::<state::AppState>() {
            // §20.1 — 앱이 닫히는 중이면 이 스레드도 끝난다.
            Some(state) if state.cancel.load(Ordering::Relaxed) => return,
            Some(_) => {}
            None => return,
        }
        if !handle.global_shortcut().is_registered(CLIPBOARD_SHORTCUT) {
            // 되찾기 실패는 조용히 넘긴다 — 1.5초마다 같은 줄을 찍을 이유가 없다.
            let _ = try_arm_clipboard_shortcut(&handle);
        }
    });
}

async fn clipboard_ingest(app: tauri::AppHandle) {
    let text = match app.clipboard().read_text() {
        Ok(t) => t,
        Err(e) => {
            let _ = app.emit(
                EVENT_CLIPBOARD_ERROR,
                format!("클립보드를 읽지 못했습니다 — {e}"),
            );
            return;
        }
    };
    if text.trim().is_empty() {
        let _ = app.emit(
            EVENT_CLIPBOARD_ERROR,
            "클립보드가 비어 있습니다.".to_string(),
        );
        return;
    }
    // kind 판별과 거부 사유는 전부 아래가 정한다 (§9.1). 여기서 문구를 짓지 않는다.
    match ingest_clipboard_flow(&app, text).await {
        Ok(card) => {
            let _ = app.emit(EVENT_CLIPBOARD_CARD, vec![card]);
        }
        Err(e) => {
            let _ = app.emit(EVENT_CLIPBOARD_ERROR, e);
        }
    }
}

/// **`commands::ingest_clipboard`의 인자가 바뀌면 고칠 곳은 이 함수 하나다.**
async fn ingest_clipboard_flow(
    app: &tauri::AppHandle,
    text: String,
) -> Result<commands::SensoryCard, String> {
    commands::ingest_clipboard(app.clone(), app.state::<state::AppState>(), text).await
}

// ─────────────────────────────────────────────────── §9.10 큐 이어받기 · §20.1 종료

/// §9.10 · T51c — 지난 실행이 큐 처리 중에 죽었으면 `open_or_init`의 `queue_recover`가
/// `running`을 `pending`으로 되돌려 둔다. **이어받는 것은 여기다.**
///
/// 큐가 비어 있으면 잠금을 곧바로 놓는다. 비어 있지 않을 때만 워커가 돌고, 그동안
/// `AppState.app` 잠금을 쥔다 — `setup_status`가 잠금을 잡지 않는 이유가 이것이다.
fn spawn_startup_drain(handle: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(STARTUP_DRAIN_DELAY).await;
        let Some(state) = handle.try_state::<state::AppState>() else {
            return;
        };
        if state.cancel.load(Ordering::Relaxed) {
            return;
        }
        let guard = state.app.lock().await;
        let Some(app) = guard.as_ref() else {
            return;
        };
        // 집을 것이 없으면 키체인도 네트워크도 건드리지 않는다.
        if app.db.queue_pending_count().unwrap_or(0) == 0 {
            return;
        }
        let worker = soul_pipeline::critique_worker::Worker {
            // §20.1 · T44 — 앱을 닫으면 이 신호로 함께 멈춘다.
            cancel: std::sync::Arc::clone(&state.cancel),
        };
        // 실패는 큐와 트레이스에 남는다 (§9.10). 건별 알림을 띄우지 않는다 (T51e).
        // `&App`은 `Send`가 아니므로 spawn된 future에 그대로 들고 있을 수 없다.
        let _ = state::run_pipeline(worker.drain(app));
    });
}

/// §20.1 · T44 — 앱이 닫히면 비평 워커도 멈추고 열어 둔 것을 닫는다.
fn shutdown(handle: &tauri::AppHandle) {
    let Some(state) = handle.try_state::<state::AppState>() else {
        return;
    };
    // 먼저 신호를 내린다. 워커는 20ms 폴링 안에 멈춘다 (`critique_worker::CANCEL_POLL_MS`).
    state.cancel.store(true, Ordering::SeqCst);
    if SHUTDOWN_DONE.swap(true, Ordering::SeqCst) {
        return;
    }

    let _ = handle.global_shortcut().unregister_all();

    // sqlite 연결과 트레이스 파일을 닫는다. 워커가 아직 잠금을 쥐고 있으면 잠깐 기다리되,
    // 종료가 이것 때문에 매달리게 두지 않는다 — 프로세스가 끝나면 어차피 정리된다.
    let _ = tauri::async_runtime::block_on(tokio::time::timeout(SHUTDOWN_LOCK_WAIT, async {
        state.app.lock().await.take();
    }));

    // §R7 — 두 번째 인스턴스를 받던 IPC 자원을 정리한다.
    tauri_plugin_single_instance::destroy(handle);
}
