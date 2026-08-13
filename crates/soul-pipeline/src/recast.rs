//! `soul recast <ingest_id> <kind>` — YouTube 항목의 kind 뒤집기 (§9.3 · §14).
//!
//! 관측은 불변이므로(§R2) 기존 `ingest`를 수정하지 않는다.
//!
//! 1. 뒤집은 `kind`로 §9.5 또는 §9.6 경로를 다시 태워 **새 `ingest` 관측**을 만든다
//! 2. 새 관측에 `"supersedes": "<이전 ULID>"`를 넣는다 (§R9). 이전 관측은 파생 계산에서 빠진다
//! 3. 이전 관측에 달렸던 `reading`은 **이월하지 않는다.** 서술이 달라졌으므로 판정도 다시 받는다
//! 4. 이전 `context`도 이월하지 않는다. 새 관측에 대해 §9.10 큐에 다시 등록한다
//! 5. 다운로드한 클립은 캐시에 남아 있으므로 **yt-dlp를 다시 호출하지 않는다** (T11j)
//!
//! 응답을 마친 뒤 뒤집으면 그 응답이 버려진다는 것을 **확인 다이얼로그로 알린다.**

use soul_core::error::{Result, SoulError};
use soul_core::ids::ObsId;
use soul_core::obs::{Kind, Layer};
use std::collections::HashSet;

pub async fn recast(
    app: &crate::App,
    id: &ObsId,
    new_kind: Kind,
) -> Result<crate::ingest::IngestOutcome> {
    let previous = app.store.read(id)?;
    let previous = previous.as_ingest().ok_or_else(|| {
        SoulError::invalid(format!(
            "{id} 은(는) ingest 관측이 아닙니다. 뒤집을 수 없습니다"
        ))
    })?;

    // 뒤집기는 §9.3의 kind 추정을 되돌리는 동작이다. 다른 경로에는 추정 자체가 없다.
    let Some(video_id) = soul_media::probe::youtube_video_id(&previous.source.origin) else {
        return Err(SoulError::invalid(format!(
            "kind 뒤집기는 YouTube 항목에만 쓸 수 있습니다 (origin: {})",
            previous.source.origin
        )));
    };
    if !matches!(new_kind, Kind::Audio | Kind::Video) {
        return Err(SoulError::invalid(format!(
            "YouTube 항목은 audio 또는 video로만 뒤집을 수 있습니다 (요청: {new_kind})"
        )));
    }
    if previous.source.kind == new_kind {
        return Err(SoulError::invalid(format!(
            "이미 {new_kind} 입니다. 뒤집을 것이 없습니다"
        )));
    }

    // 1단계 — 뒤집은 kind로 §9.3 경로를 다시 탄다. `forced`를 주므로 추정이 아니다.
    //         클립이 캐시에 있으면 `download_clip`이 yt-dlp를 부르지 않는다 (T11j).
    let plan = crate::ingest::resolve_youtube(app, &video_id, Some(new_kind)).await?;

    // 2단계 — `supersedes`. 3·4단계는 **아무것도 복사하지 않는 것**으로 성립한다:
    //         이전 `reading`·`context`는 그대로 두고 새 관측에 이월하지 않으며,
    //         `run`이 새 ID로 §9.10 큐에 다시 등록한다 (T11i).
    crate::ingest::run(app, plan, Some(id.clone()), None).await
}

/// 뒤집기로 버려질 응답 수. 확인 다이얼로그가 이 값을 보여준다.
///
/// 이전 `ingest`에 달린 `sensory` 응답 + 그 `ingest`의 `context`들에 달린 `cultural` 응답.
/// 문화 응답은 `ingest`가 아니라 `context`를 가리키므로 2단으로 센다 (§12.6, T55c).
pub fn discarded_readings(app: &crate::App, id: &ObsId) -> Result<usize> {
    let set = app.store.load_set()?;
    let contexts: HashSet<&ObsId> = set
        .contexts()
        .into_iter()
        .filter(|c| &c.target == id)
        .map(|c| &c.id)
        .collect();
    let n = set
        .readings()
        .into_iter()
        .filter(|r| match r.layer {
            Layer::Sensory => &r.target == id,
            Layer::Cultural => contexts.contains(&r.target),
        })
        .count();
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::testkit::{err_of, record_text, temp_app, write_context};
    use crate::reading::record_reading;
    use soul_core::obs::Verdict;

    // ────────────────────────────────────────────── 버려질 응답 수 (§9.3 확인 다이얼로그)

    #[test]
    fn discarded_counts_both_layers_through_the_two_stage_join() {
        let app = temp_app("discard");
        let ingest = record_text(&app, "뒤집을 대상", None);
        let other = record_text(&app, "무관한 대상", None);

        // 감각 응답 1건.
        record_reading(
            &app,
            Layer::Sensory,
            &ingest,
            Verdict::Yes,
            None,
            None,
            None,
        )
        .unwrap();
        // 문화 응답 2건 — context를 target으로 한다.
        let ctx1 = write_context(&app, &ingest, "첫 비평", 2);
        let ctx2 = write_context(&app, &ingest, "다시 만든 비평", 2);
        record_reading(&app, Layer::Cultural, &ctx1, Verdict::No, None, None, None).unwrap();
        record_reading(&app, Layer::Cultural, &ctx2, Verdict::Yes, None, None, None).unwrap();

        // 다른 항목의 응답은 세지 않는다.
        record_reading(&app, Layer::Sensory, &other, Verdict::Yes, None, None, None).unwrap();
        let other_ctx = write_context(&app, &other, "무관한 비평", 2);
        record_reading(
            &app,
            Layer::Cultural,
            &other_ctx,
            Verdict::Yes,
            None,
            None,
            None,
        )
        .unwrap();

        assert_eq!(discarded_readings(&app, &ingest).unwrap(), 3);
        assert_eq!(discarded_readings(&app, &other).unwrap(), 2);
    }

    #[test]
    fn discarded_is_zero_before_any_answer() {
        // 첫 카드에서 뒤집으면 버려지는 것이 없다 — 그래서 그 시점을 권한다 (§9.3).
        let app = temp_app("discard-zero");
        let ingest = record_text(&app, "아직 답하지 않은 대상", None);
        assert_eq!(discarded_readings(&app, &ingest).unwrap(), 0);
        write_context(&app, &ingest, "비평", 2);
        assert_eq!(
            discarded_readings(&app, &ingest).unwrap(),
            0,
            "context만 있고 응답이 없으면 버려질 것이 없다"
        );
    }

    #[test]
    fn discarded_ignores_readings_on_other_items_contexts() {
        // 2단 조인을 한 단으로 줄이면 남의 context에 달린 응답이 섞인다 (T55c).
        let app = temp_app("discard-join");
        let a = record_text(&app, "A", None);
        let b = record_text(&app, "B", None);
        let ctx_b = write_context(&app, &b, "B의 비평", 2);
        record_reading(
            &app,
            Layer::Cultural,
            &ctx_b,
            Verdict::Yes,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(discarded_readings(&app, &a).unwrap(), 0);
        assert_eq!(discarded_readings(&app, &b).unwrap(), 1);
    }

    // ──────────────────────────────────────────────────── 뒤집기 자체 (§9.3)

    #[tokio::test]
    async fn recast_refuses_non_youtube_items() {
        // 로컬 파일·텍스트에는 kind 추정이 없으므로 뒤집을 것도 없다.
        let app = temp_app("recast-nonyt");
        let ingest = record_text(&app, "붙여넣은 글", None);
        let err = err_of(recast(&app, &ingest, Kind::Audio).await);
        assert!(err.contains("YouTube 항목에만"), "{err}");
        assert_eq!(app.store.count().unwrap(), 1, "관측을 만들면 안 된다");
    }

    #[tokio::test]
    async fn recast_refuses_text_and_image_targets() {
        let app = temp_app("recast-kind");
        let ingest = record_youtube(&app, Kind::Video);
        for kind in [Kind::Text, Kind::Image] {
            let err = err_of(recast(&app, &ingest, kind).await);
            assert!(err.contains("audio 또는 video"), "{err}");
        }
    }

    #[tokio::test]
    async fn recast_refuses_a_no_op_flip() {
        let app = temp_app("recast-same");
        let ingest = record_youtube(&app, Kind::Video);
        let err = err_of(recast(&app, &ingest, Kind::Video).await);
        assert!(err.contains("이미 video"), "{err}");
        assert_eq!(app.store.count().unwrap(), 1);
    }

    #[tokio::test]
    async fn recast_rejects_non_ingest_targets() {
        let app = temp_app("recast-target");
        let ingest = record_text(&app, "대상", None);
        let reading = record_reading(
            &app,
            Layer::Sensory,
            &ingest,
            Verdict::Yes,
            None,
            None,
            None,
        )
        .unwrap();
        let err = err_of(recast(&app, &reading, Kind::Audio).await);
        assert!(err.contains("ingest 관측이 아닙니다"), "{err}");
    }

    /// 뒤집기 이후의 상태 — `run`을 태우지 않고 결과만 조립해 §9.3 3~5단계를 검증한다.
    /// (전체 흐름은 네트워크가 필요하므로 `tests/`의 `SOUL_E2E` 게이트에 있다.)
    #[test]
    fn a_recast_observation_supersedes_and_carries_nothing_over() {
        let app = temp_app("recast-shape");
        let old = record_youtube(&app, Kind::Video);
        record_reading(&app, Layer::Sensory, &old, Verdict::Yes, None, None, None).unwrap();
        let ctx = write_context(&app, &old, "옛 비평", 2);
        record_reading(&app, Layer::Cultural, &ctx, Verdict::No, None, None, None).unwrap();
        assert_eq!(discarded_readings(&app, &old).unwrap(), 2);

        // `run`의 마지막 단계와 같은 기록을 한다.
        let new = record_text(&app, "뒤집은 서술", Some(old.clone()));

        // 2단계 — supersedes.
        let obs = app.store.read(&new).unwrap();
        assert_eq!(obs.as_ingest().unwrap().supersedes, Some(old.clone()));
        let set = app.store.load_set().unwrap();
        let active: Vec<&ObsId> = set.active_ingests().iter().map(|i| &i.id).collect();
        assert_eq!(active, vec![&new], "§R9 — 이전 관측은 파생 계산에서 빠진다");

        // 3·4단계 — 이전 reading·context를 이월하지 않는다 (T11i).
        assert!(
            set.latest_reading_for(&new, Layer::Sensory).is_none(),
            "감각 응답이 이월되었다"
        );
        assert!(
            set.latest_context_for(&new).is_none(),
            "문화 맥락이 이월되었다"
        );
        // 이전 관측에 달린 것들은 그대로 남는다 — 관측은 불변이다 (§R2).
        assert!(set.latest_reading_for(&old, Layer::Sensory).is_some());
        assert_eq!(discarded_readings(&app, &old).unwrap(), 2);

        // 4단계 — 새 ID로 큐에 다시 등록된다.
        let queued: Vec<String> = app
            .db
            .queue_items(None)
            .unwrap()
            .into_iter()
            .map(|i| i.ingest_id)
            .collect();
        assert!(queued.contains(&new.to_string()));
    }

    /// §9.3 형태의 YouTube `ingest`를 하나 기록한다.
    fn record_youtube(app: &crate::App, kind: Kind) -> ObsId {
        crate::ingest::testkit::record_youtube(app, kind, "dQw4w9WgXcQ")
    }
}
