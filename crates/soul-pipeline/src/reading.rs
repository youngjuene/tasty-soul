//! ○/× 응답 기록 (§6.3 · §13 화면 2 · 2.1).
//!
//! - `yes` → `prose: null`, `divergence: null` (T7)
//! - `no` + 문장 → `divergence` 계산 (T6). 문장 없이 확인해도 `verdict: no`는 저장
//! - **카드를 무시하면 `reading` 관측을 만들지 않는다.** 중간 선택지를 두지 않는다 (T59)
//! - `sensory`의 `target`은 `ingest` ID, `cultural`의 `target`은 **`context` ID** (§12.6)

use soul_core::derived::divergence::reading_divergence;
use soul_core::error::{Result, SoulError};
use soul_core::git;
use soul_core::ids::ObsId;
use soul_core::lock::WriteLock;
use soul_core::obs::{Layer, Observation, Reading, Verdict};

pub async fn record(
    app: &crate::App,
    target: &ObsId,
    layer: Layer,
    verdict: Verdict,
    prose: Option<String>,
) -> Result<ObsId> {
    let set = app.store.load_set()?;

    // 대상 텍스트는 layer가 정한다 (§6.3).
    //   sensory  → 대상 `ingest`의 `machine.prose`
    //   cultural → 대상 `context`의 `critique`   ← ingest가 아니다 (§12.6의 2단 조인, T55c)
    let target_text = match layer {
        Layer::Sensory => set
            .get(target)
            .and_then(|o| o.as_ingest())
            .map(|i| i.machine.prose.clone())
            .ok_or_else(|| {
                SoulError::invalid(format!(
                    "sensory reading의 target은 ingest여야 합니다: {target}"
                ))
            })?,
        Layer::Cultural => set
            .get(target)
            .and_then(|o| o.as_context())
            .map(|c| c.critique.clone())
            .ok_or_else(|| {
                SoulError::invalid(format!(
                    "cultural reading의 target은 context여야 합니다 (§12.6의 2단 조인): {target}"
                ))
            })?,
    };

    let prose = normalize_prose(verdict, prose);

    // `divergence`는 **`prose`가 있을 때만** 계산하는 보조 지표다 (§6.3).
    // 주 신호는 `verdict`이므로 여기서 실패하면 값이 아니라 관측 자체를 만들지 않는다 (§15).
    let mut divergence = None;
    let mut prose_vec: Option<Vec<f32>> = None;
    if let Some(text) = &prose {
        let client = app.openai()?;
        let embedder = crate::ingest::embedder(app, &client);
        // 임베딩 공간도 layer를 따른다. 섞으면 §12.7이 깨진다 (T49).
        let target_vec = embedder.get(&target_text).await?;
        let user_vec = embedder.get(text).await?;
        divergence = Some(reading_divergence(&target_vec, &user_vec));
        prose_vec = Some(user_vec);
    }

    record_reading(
        app,
        layer,
        target,
        verdict,
        prose,
        divergence,
        prose_vec.as_deref(),
    )
}

/// 화면 2.1 대기 목록 — **미응답 문화 카드는 앱 재시작 후에도 남는다** (T53).
/// 화면 2의 감각 카드는 영속화하지 않는다 (§13 화면 2).
pub fn pending_cultural(app: &crate::App) -> Result<Vec<PendingCard>> {
    let set = app.store.load_set()?;
    // 대기 목록은 관측 로그에서 매번 다시 만든다. 그래서 앱을 닫아도 남는다 (T53).
    let mut out = Vec::new();
    for c in set.contexts() {
        // 뒤집기·재분석으로 supersede된 ingest의 문화 카드는 이월하지 않는다 (§9.3, T11i).
        let target_is_active = set.active_ingests().iter().any(|i| i.id == c.target);
        if !target_is_active {
            continue;
        }
        // 재요청으로 `context`가 여럿이면 **최신 것만** 묻는다 (§12.6, T55b).
        if set.latest_context_for(&c.target).map(|l| &l.id) != Some(&c.id) {
            continue;
        }
        if set.latest_reading_for(&c.id, Layer::Cultural).is_some() {
            continue;
        }
        out.push(PendingCard {
            context_id: c.id.clone(),
            ingest_id: c.target.clone(),
            critique: c.critique.clone(),
            lineage: c.lineage.clone(),
            grounded: c.grounded,
            sources: c.sources.clone(),
        });
    }
    Ok(out)
}

pub struct PendingCard {
    pub context_id: ObsId,
    pub ingest_id: ObsId,
    pub critique: String,
    pub lineage: Vec<String>,
    pub grounded: bool,
    pub sources: Vec<soul_core::obs::SourceRef>,
}

// ─────────────────────────────────────────────────────────────────── 내부

/// §6.3 — `yes`면 `prose`는 반드시 `null`이다 (T7).
///
/// 화면 2·2.1에는 중간 선택지가 없으므로(T59) `no` + 빈 문장은 "건너뛰기"이며,
/// 그 경우에도 `verdict: no`는 저장한다 (§13 화면 2).
fn normalize_prose(verdict: Verdict, prose: Option<String>) -> Option<String> {
    match verdict {
        Verdict::Yes => None,
        Verdict::No => prose
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
    }
}

/// 관측 기록 + 커밋 1개 (§R8). **재렌더하지 않는다** (T21b).
///
/// 네트워크를 쓰지 않는다 — 오프라인 테스트가 이 함수를 직접 부른다.
pub(crate) fn record_reading(
    app: &crate::App,
    layer: Layer,
    target: &ObsId,
    verdict: Verdict,
    prose: Option<String>,
    divergence: Option<f64>,
    prose_vec: Option<&[f32]>,
) -> Result<ObsId> {
    let (id, ts, schema) = soul_core::obs::new_header();
    let obs = Observation::Reading(Reading {
        id: id.clone(),
        ts,
        schema,
        layer,
        target: target.clone(),
        verdict,
        prose,
        divergence,
    });
    // 구조 불변식은 `Store::append`가 기록 직전에 검사한다 (§6.3).
    {
        // §R7 — 쓰기는 락 안에서. 실패하면 즉시 에러다(대기하지 않는다).
        let _lock = WriteLock::acquire(&app.paths.soul())?;
        let path = app.store.append(&obs)?;
        git::commit_paths(&app.paths.soul(), &[&path], &format!("reading {id}"))?;
    }
    // §12.7 불변식 — `reading.prose` 벡터는 **텍스트 키 캐시에만** 둔다 (`Embedder::get`이
    // 이미 넣었다). ID 색인(`obs_vec`/`critique_vec`)에 넣지 않는다.
    //
    // ID 색인은 `cluster()`·`soul_similar` 이 "투입한 대상들"의 목록으로 쓰는 곳이다.
    // 사용자 정정문이 거기 섞이면 모든 소비자가 매번 활성 ingest ID로 걸러야만 안전해진다.
    // divergence·coherence 는 텍스트로 조회하므로 캐시만으로 충분하고,
    // `soul-cli` 의 rebuild backfill 도 같은 규칙을 따른다 — 두 경로가 어긋나면 §R2가 깨진다.
    let _ = prose_vec;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::testkit::{record_text, temp_app, write_context};

    // ────────────────────────────────────────────── yes/no 불변식 (§6.3, T6·T7)

    #[test]
    fn yes_drops_prose_and_divergence() {
        // T7 — `yes` → prose null, divergence null. 입력이 와도 버린다.
        assert_eq!(normalize_prose(Verdict::Yes, Some("그렇다".into())), None);
        assert_eq!(normalize_prose(Verdict::Yes, None), None);
    }

    #[test]
    fn no_without_a_sentence_is_still_recorded() {
        // §13 화면 2 — 입력 없이 확인해도 `verdict: no`는 저장한다.
        assert_eq!(normalize_prose(Verdict::No, None), None);
        assert_eq!(normalize_prose(Verdict::No, Some("   \n ".into())), None);
        assert_eq!(
            normalize_prose(Verdict::No, Some("  서늘한 거리감 ".into())),
            Some("서늘한 거리감".to_string())
        );
    }

    #[test]
    fn a_yes_reading_is_written_with_both_fields_null() {
        let app = temp_app("reading-yes");
        let ingest = record_text(&app, "차갑고 정돈된 실내", None);
        let id = record_reading(
            &app,
            Layer::Sensory,
            &ingest,
            Verdict::Yes,
            None,
            None,
            None,
        )
        .unwrap();

        let obs = app.store.read(&id).unwrap();
        let r = obs.as_reading().unwrap();
        assert_eq!(r.verdict, Verdict::Yes);
        assert!(r.prose.is_none());
        assert!(r.divergence.is_none());
        assert_eq!(r.target, ingest);

        let ts = obs.ts();
        let json = std::fs::read_to_string(app.paths.observation_file(ts, &id)).unwrap();
        assert!(json.contains("\"prose\": null"), "{json}");
        assert!(json.contains("\"divergence\": null"), "{json}");
    }

    #[test]
    fn yes_with_prose_is_rejected_by_the_schema() {
        // §6.3의 불변식이 저장 직전에 걸린다 — 우회 경로를 만들지 않았음을 못박는다.
        let app = temp_app("reading-yes-bad");
        let ingest = record_text(&app, "대상", None);
        let err = record_reading(
            &app,
            Layer::Sensory,
            &ingest,
            Verdict::Yes,
            Some("문장".into()),
            None,
            None,
        )
        .unwrap_err();
        assert!(format!("{err}").contains("prose"), "{err}");
        assert_eq!(app.store.count().unwrap(), 1, "파일을 만들면 안 된다");
    }

    #[test]
    fn divergence_needs_prose() {
        // §6.3 — prose 없이 divergence를 둘 수 없다.
        let app = temp_app("reading-div");
        let ingest = record_text(&app, "대상", None);
        assert!(record_reading(
            &app,
            Layer::Sensory,
            &ingest,
            Verdict::No,
            None,
            Some(0.4),
            None
        )
        .is_err());
    }

    #[test]
    fn a_no_reading_keeps_prose_and_divergence_and_links_its_vector() {
        // T6 — `no` + 문장 → divergence가 붙는다.
        let app = temp_app("reading-no");
        let ingest = record_text(&app, "차갑고 정돈된 실내", None);
        let id = record_reading(
            &app,
            Layer::Sensory,
            &ingest,
            Verdict::No,
            Some("향수 아니고 오히려 좀 서늘한 거리감".into()),
            Some(0.41),
            Some(&[0.0f32, 1.0]),
        )
        .unwrap();

        let obs = app.store.read(&id).unwrap();
        let r = obs.as_reading().unwrap();
        assert_eq!(r.verdict, Verdict::No);
        assert_eq!(r.divergence, Some(0.41));
        assert!(r.prose.is_some());
        // §12.7 불변식 — 정정문 벡터는 **ID 색인에 들어가지 않는다.**
        // 색인은 `cluster()`·`soul_similar` 이 "투입한 대상들"로 읽는 목록이다.
        assert!(app
            .db
            .obs_vec_get(soul_core::db::embed_cache::Space::Object, id.as_str())
            .unwrap()
            .is_none());
        assert!(app
            .db
            .obs_vec_get(soul_core::db::embed_cache::Space::Critique, id.as_str())
            .unwrap()
            .is_none());
    }

    #[test]
    fn reading_prose_never_enters_the_id_index() {
        // §12.7 · T49 — 두 공간을 섞지 않는다.
        let app = temp_app("reading-cultural-space");
        let ingest = record_text(&app, "대상", None);
        let ctx = write_context(&app, &ingest, "비평문", 2);
        let id = record_reading(
            &app,
            Layer::Cultural,
            &ctx,
            Verdict::No,
            Some("그것 때문은 아니다".into()),
            Some(0.6),
            Some(&[1.0f32, 0.0]),
        )
        .unwrap();
        // cultural 정정문도 마찬가지로 ID 색인에 남기지 않는다.
        // divergence·coherence 는 텍스트 키 캐시로 조회하므로 그것으로 충분하다.
        assert!(app
            .db
            .obs_vec_get(soul_core::db::embed_cache::Space::Critique, id.as_str())
            .unwrap()
            .is_none());
        assert!(app
            .db
            .obs_vec_get(soul_core::db::embed_cache::Space::Object, id.as_str())
            .unwrap()
            .is_none());
    }

    #[test]
    fn recording_a_reading_makes_one_commit_and_does_not_rerender() {
        // §R8 · T21b — `reading`도 재렌더를 유발하지 않는다.
        let app = temp_app("reading-commit");
        std::fs::write(app.paths.soul_md(), "# SOUL\n").unwrap();
        let ingest = record_text(&app, "대상", None);
        // 저장소 초기화 커밋은 이 검사의 대상이 아니다 — **증분**으로 센다.
        let log_before = git::log_messages(&app.paths.soul(), 100).unwrap().len();
        let id = record_reading(
            &app,
            Layer::Sensory,
            &ingest,
            Verdict::Yes,
            None,
            None,
            None,
        )
        .unwrap();

        let log = git::log_messages(&app.paths.soul(), 100).unwrap();
        assert_eq!(log.len(), log_before + 1, "reading 1건 = 커밋 1개 (§R8)");
        assert_eq!(log[0], format!("reading {id}"));
        assert_eq!(
            std::fs::read_to_string(app.paths.soul_md()).unwrap(),
            "# SOUL\n"
        );
    }

    // ─────────────────────────────────────────────── 대상 타입 검사 (§12.6)

    #[tokio::test]
    async fn sensory_target_must_be_an_ingest_and_cultural_must_be_a_context() {
        let app = temp_app("reading-target");
        let ingest = record_text(&app, "대상", None);
        let ctx = write_context(&app, &ingest, "비평문", 2);

        // cultural이 ingest를 가리키면 셀이 영원히 null이 된다 (T55c).
        let err = record(&app, &ingest, Layer::Cultural, Verdict::Yes, None)
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("context여야"), "{err}");

        // 반대 방향도 막는다.
        let err = record(&app, &ctx, Layer::Sensory, Verdict::Yes, None)
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("ingest여야"), "{err}");
    }

    #[tokio::test]
    async fn yes_never_touches_the_network() {
        // `yes`는 임베딩이 필요 없다. `app.openai()`가 아직 없어도 기록되어야 한다.
        let app = temp_app("reading-offline-yes");
        let ingest = record_text(&app, "대상", None);
        let id = record(&app, &ingest, Layer::Sensory, Verdict::Yes, None)
            .await
            .expect("yes 응답은 오프라인으로 기록된다");
        assert_eq!(
            app.store.read(&id).unwrap().as_reading().unwrap().verdict,
            Verdict::Yes
        );
    }

    #[tokio::test]
    async fn no_without_a_sentence_never_touches_the_network_either() {
        let app = temp_app("reading-offline-no");
        let ingest = record_text(&app, "대상", None);
        let id = record(
            &app,
            &ingest,
            Layer::Sensory,
            Verdict::No,
            Some("  ".into()),
        )
        .await
        .expect("건너뛴 no도 오프라인으로 기록된다");
        let obs = app.store.read(&id).unwrap();
        let r = obs.as_reading().unwrap();
        assert_eq!(r.verdict, Verdict::No);
        assert!(r.prose.is_none());
        assert!(r.divergence.is_none());
    }

    // ─────────────────────────────────────────────────── 화면 2.1 대기 목록 (T53)

    #[test]
    fn pending_cultural_lists_unanswered_contexts_in_ulid_order() {
        let app = temp_app("pending");
        let a = record_text(&app, "투입 A", None);
        let b = record_text(&app, "투입 B", None);
        let ctx_a = write_context(&app, &a, "비평 A", 2);
        let ctx_b = write_context(&app, &b, "비평 B", 1);

        let pending = pending_cultural(&app).unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].context_id, ctx_a);
        assert_eq!(pending[0].ingest_id, a);
        assert_eq!(pending[0].critique, "비평 A");
        assert!(pending[0].grounded, "근거 2건이면 grounded");
        assert_eq!(pending[0].sources.len(), 2);
        assert_eq!(pending[1].context_id, ctx_b);
        assert!(!pending[1].grounded, "근거 1건이면 grounded가 아니다");

        // 답한 카드는 목록에서 빠진다.
        record_reading(
            &app,
            Layer::Cultural,
            &ctx_a,
            Verdict::Yes,
            None,
            None,
            None,
        )
        .unwrap();
        let pending = pending_cultural(&app).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].context_id, ctx_b);
    }

    #[test]
    fn pending_cultural_survives_a_restart() {
        // T53 — 미응답 카드는 앱 재시작 후에도 남는다. 관측 로그에서 매번 다시 만든다.
        let app = temp_app("pending-restart");
        let ingest = record_text(&app, "투입", None);
        let ctx = write_context(&app, &ingest, "비평", 2);
        assert_eq!(pending_cultural(&app).unwrap().len(), 1);

        // 앱을 다시 연다 — 같은 루트에 새 Store·Db.
        let reopened = crate::App {
            paths: app.paths.clone(),
            config: app.config.clone(),
            db: soul_core::db::Db::open(&app.paths.derived_db()).unwrap(),
            store: soul_core::obs::Store::new(app.paths.clone()),
            tracer: None,
            // 다시 여는 흉내일 뿐 스켈레톤을 만들지는 않았다.
            created_soul_md: false,
        };
        let pending = pending_cultural(&reopened).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].context_id, ctx);
    }

    #[test]
    fn pending_cultural_skips_superseded_and_stale_contexts() {
        let app = temp_app("pending-stale");
        let old = record_text(&app, "원본", None);
        let stale_ctx = write_context(&app, &old, "옛 비평", 2);
        // 뒤집기 — 이전 context를 이월하지 않는다 (T11i).
        let new = record_text(&app, "뒤집은 원본", Some(old.clone()));
        assert!(
            pending_cultural(&app).unwrap().is_empty(),
            "supersede된 ingest의 카드는 남지 않는다"
        );
        let _ = stale_ctx;

        // 새 관측에 대해 context가 두 번 생기면(§14 --redo) 최신 것만 묻는다 (T55b).
        let first = write_context(&app, &new, "첫 비평", 2);
        let second = write_context(&app, &new, "다시 만든 비평", 2);
        let pending = pending_cultural(&app).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].context_id, second);
        assert_ne!(pending[0].context_id, first);
    }
}
