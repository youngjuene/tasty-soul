//! 앱 전역 상태.

use std::sync::Arc;

/// 모든 필드가 "비어 있음"에서 시작하므로 `AppState::default()`로 만들 수 있다.
/// 셸(`lib::run`)이 필드를 하나씩 채우는 대신 이것을 쓰면 필드가 늘어도 깨지지 않는다.
#[derive(Default)]
pub struct AppState {
    pub app: tokio::sync::Mutex<Option<soul_pipeline::App>>,
    /// 비평 워커 취소 신호. 앱 종료 시 내린다 (§20.1, T44).
    pub cancel: Arc<std::sync::atomic::AtomicBool>,
    /// 미응답 감각 카드. **영속화하지 않는다** (§13 화면 2).
    pub sensory_cards: tokio::sync::Mutex<Vec<commands::SensoryCard>>,
    /// §13 화면 4 — 지금 화면에 떠 있는 성찰 제안.
    ///
    /// `approve_proposal`은 **`reflect`가 만든 바로 그 제안**을 기록해야 한다.
    /// 다시 만들면 `blocks.profile.from_hash`가 제안 시점의 값이 아니게 되어
    /// §11.2의 가드레일이 무의미해지고, 모델을 한 번 더 부르게 된다.
    /// 영속화하지 않는다 — 앱을 닫으면 사라지고 다음 `reflect`가 새로 만든다.
    pub proposal: tokio::sync::Mutex<Option<soul_pipeline::reflect_flow::Proposal>>,
    /// §3 — 이번 실행이 최초 실행인가.
    ///
    /// `lib::run`이 `App::open_or_init` **전에** 정한다: `SOUL.md`가 없었으면 참이다.
    /// 열고 나서 보면 스켈레톤이 이미 만들어져 있어 구별할 수 없다.
    pub first_run: std::sync::atomic::AtomicBool,
    /// §3 — `App::open_or_init` 실패 사유.
    ///
    /// **여기서 패닉하지 않는다.** 창은 떠야 하고, 사유는 `setup_status`가 그대로 전달해
    /// 사용자가 «다시 시도»를 누를 수 있어야 한다 (`App.tsx`의 부팅 오류 화면).
    /// 다시 열기에 성공하면 `setup_status`가 이 값을 지운다.
    pub open_error: std::sync::Mutex<Option<String>>,
}

use crate::commands;

/// **`&App`을 await 너머로 들고 가지 않고 async 파이프라인을 부르는 유일한 방법.**
///
/// `soul_pipeline::App` 안에는 `rusqlite::Connection`이 있고, 그것은 `Send`지만 `Sync`가
/// **아니다**(`RefCell`을 쓴다). 그래서 `&App`은 `Send`가 아니고, `&App`을 await 너머로
/// 들고 있는 future도 `Send`가 아니다. Tauri의 async 커맨드는 `Send` future만 받으므로
/// `doctor::run(app).await` 같은 호출을 커맨드 본문에 그대로 쓰면 컴파일되지 않는다.
///
/// 여기서는 future를 **지금 스레드에서 끝까지 돌린다.** `block_in_place`가 이 워커 스레드를
/// 런타임에서 잠시 떼어 놓으므로 다른 태스크는 다른 스레드에서 계속 돈다. 바깥 커맨드
/// future에는 `&App`이 남지 않으므로 `Send`가 유지된다.
///
/// **UI 스레드를 막지 않는다** — 커맨드는 async 런타임에서 돌고 메인 스레드는 여기에 없다.
///
/// ## 쓰는 법
///
/// ```ignore
/// let guard = pipeline_support::app_lock(&state).await?;
/// let app = pipeline_support::borrow(&guard)?;
/// crate::state::run_pipeline(soul_pipeline::doctor::run(app, probe_models))
///     .map_err(|e| e.to_string())
/// ```
///
/// **중첩해서 부르지 말 것.** 이 안에서 도는 코드는 이미 블로킹 스레드 위에 있으므로
/// 동기 작업은 `block_in_place` 없이 그냥 부르면 된다.
pub fn run_pipeline<F: std::future::Future>(fut: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
}
