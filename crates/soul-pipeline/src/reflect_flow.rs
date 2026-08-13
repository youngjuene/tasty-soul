//! 성찰 흐름 (§11.2 · §13 화면 4).
//!
//! 에이전트는 `soul/SOUL.next.md`에 쓰고 앱이 diff 뷰를 띄운다.
//! **사용자 승인 시에만** `soul_delta` 기록 → 재렌더 → git 커밋.
//! 거절 시 관측을 기록하지 않고 트레이스에만 남긴다.

use soul_core::derived::Derived;
use soul_core::error::{Result, SoulError};
use soul_core::ids::ObsId;
use soul_core::obs::SoulDelta;
use soul_core::obs::{Axis, AxisDelta, BlockDelta, Observation};
use soul_core::soulmd::{self, RenderInput};
use soul_core::time::Ts;
use soul_core::trace::TraceEntry;

/// §8.2 템플릿의 유일한 `soul:neg` 블록 id. `soul_delta.blocks`의 키이기도 하다.
const PROFILE_BLOCK: &str = "profile";

/// `soul:human` 마커의 머리글자. 순수 문단 경로의 마지막 방어선이다 (§D4, 아래 참고).
const HUMAN_MARKER: &str = "soul:human";

pub struct Proposal {
    pub delta: SoulDelta,
    /// `SOUL.next.md` 전문 (diff 좌우 **표시**용).
    ///
    /// **편집 대상으로 주지 말 것.** 전문에는 `soul:human`이 들어 있다 — 화면에 보여주는
    /// 것은 목적지가 로컬 화면이라 §D4 대상이 아니지만, 그것을 편집 상자에 넣으면
    /// 사용자가 고친 전문이 승인 경로로 되돌아와 관측에 실린다 (§18-4·T29).
    pub next_md: String,
    pub current_md: String,
    /// 편집 상자가 바인딩하는 값 — 지금 `SOUL.md`의 `profile` 블록 본문**만**이다.
    pub current_profile_text: String,
    /// 편집 상자가 바인딩하는 값 — 제안된 `profile` 블록 본문**만**이다.
    ///
    /// 제안이 `profile`을 건드리지 않았으면 `current_profile_text`와 같다. 축만 바꾸는
    /// 제안에 사람이 문장을 얹을 때 편집의 출발점은 지금의 본문이어야 하기 때문이다.
    pub proposed_profile_text: String,
}

pub async fn propose(app: &crate::App, force: bool) -> Result<Option<Proposal>> {
    let set = app.store.load_set()?;
    let th = &app.config.thresholds;

    // §11.2 트리거 — 마지막 `soul_delta` 이후 `ingest`가 임계치만큼 쌓였는가.
    // `--force`는 판정을 무시한다 (§14 `soul reflect --force`).
    if !force && !soul_agent::reflect::should_trigger(&set, th.reflect_trigger_ingests) {
        return Ok(None);
    }

    let model = app.config.models.reflect.clone();
    if model.trim().is_empty() {
        return Err(soul_core::error::SoulError::config(
            "models.reflect 가 비어 있습니다. `soul doctor`로 모델을 고르세요 (§9.9)",
        ));
    }

    let derived = app.derived()?;
    let client = app.openai()?;
    let outcome = soul_agent::reflect::run(
        &set,
        &app.paths,
        &client,
        &model,
        th,
        &derived,
        app.tracer.as_ref(),
    )
    .await?;

    let Some(delta) = outcome.proposal else {
        // 제안이 없으면 지난번 대기본을 남겨 두지 않는다 — 화면 4가 낡은 diff를 띄운다.
        remove_next(app)?;
        return Ok(None);
    };

    // 좌우 diff의 왼쪽. 파일이 없으면 빈 문자열이다 (§13 화면 4).
    let current_md = std::fs::read_to_string(app.paths.soul_md()).unwrap_or_default();
    let next_md = render_next(&derived, &delta, &current_md)?;
    // 화면 4의 편집 상자에 실릴 값. 전문이 아니라 `profile` 본문만이다 (§D4).
    let (current_profile_text, proposed_profile_text) = profile_texts(&delta, &current_md)?;

    // **커밋 대상이 아니다.** `.gitignore`에 있으므로 `git status`에도 안 뜬다 (§3, T20).
    // 쓰기 락을 걸지 않는다 — 관측도 `SOUL.md`도 아니어서 §R7이 지키려는 대상이 아니고,
    // 비평 워커가 관측을 쓰는 동안 제안 표시가 실패하면 화면 4를 열 수 없게 된다.
    std::fs::write(app.paths.soul_next_md(), next_md.as_bytes())?;

    Ok(Some(Proposal {
        delta,
        next_md,
        current_md,
        current_profile_text,
        proposed_profile_text,
    }))
}

/// 승인. `modified_text`가 있으면 "수정 후 승인"이다 — 수정된 텍스트로 기록한다 (§13 화면 4).
///
/// `modified_text`는 **`profile` 블록 본문**이다. 문서 전문을 주면 그 안의 `profile`
/// 블록만 쓰이고, `profile` 블록이 없으면 에러다 — 전문을 그대로 본문 삼지 않는다 (§D4).
///
/// 순서 (§R8의 재렌더 계기 1):
/// 락 → `soul_delta` 기록 + 커밋 → 락 해제 → 파생값 재계산 → 재렌더 + `render <T_ref>` 커밋
/// → `SOUL.next.md` 삭제.
///
/// 재렌더는 `rebuild::render_soul_md`가 한다. **그 함수가 자기 락을 잡으므로**
/// 관측 커밋용 락은 그 전에 놓는다 (flock은 같은 프로세스라도 두 번 잡히지 않는다).
/// `soul:human` 이월도 그 함수 안에 있다 — 여기서 다시 구현하지 않는다 (§R2, §18-5, T4).
pub async fn approve(
    app: &crate::App,
    proposal: &Proposal,
    modified_text: Option<&str>,
) -> Result<ObsId> {
    let paths = &app.paths;
    let soul_dir = paths.soul();

    // "수정 후 승인" — 사용자가 고친 텍스트가 `blocks.profile.to_text`가 된다.
    // 관측 ID와 `ts`, `window`는 제안 시점 그대로 둔다: `window.to` 이후에 들어온 관측이
    // 다음 성찰 창에 빠짐없이 들어가야 하기 때문이다 (§11.2의 `window.from` 정의).
    let delta = match modified_text {
        Some(text) => with_modified_profile(&proposal.delta, paths, text)?,
        None => proposal.delta.clone(),
    };

    // §11.2 가드레일 — **쓰기 직전의 마지막 관문.** 제안 경로든 "수정 후 승인"이든
    // 여기를 지난다. 제안 때만 검사하면 "수정 후 승인"이 그 검사를 한 번도 거치지 않는
    // 우회로가 되고, §11.2는 장식이 된다.
    if let Some(b) = delta.blocks.get(PROFILE_BLOCK) {
        validate_profile_text(&b.to_text)?;
    }

    let obs = Observation::SoulDelta(delta);
    let id = obs.id().clone();

    {
        // §R7 — 쓰기는 락 안에서. 실패하면 대기하지 않고 즉시 에러다 (§15).
        let _lock = soul_core::lock::WriteLock::acquire(&soul_dir)?;
        soul_core::git::ensure_repo(&soul_dir)?;
        // 구조 불변식(§6.6의 축 이름·`morphology_delta`)은 `append`가 검사한다.
        let written = app.store.append(&obs)?;
        // §R8 — 쓰기 1회 = 커밋 1개. 메시지는 `<type> <ULID>`.
        soul_core::git::commit_paths(
            &soul_dir,
            &[&written],
            &format!("{} {}", obs.type_name(), id),
        )?;
    }

    // 파생값 재계산 — 방금 기록한 `axis_delta`가 `offset`에 반영된다 (§12.1).
    // 네트워크를 쓰지 않는다: 임베딩은 캐시에서만 읽는다.
    app.store.invalidate();
    let derived = app.derived()?;
    let t_ref = derived
        .t_ref
        // 관측이 없는 문서에는 `T_ref`가 없다. 커밋 메시지는 파생값이 아니므로
        // 그때만 벽시계를 쓴다 (§R8의 주석과 같은 취지).
        .unwrap_or_else(Ts::now)
        .to_rfc3339_millis();
    soul_core::rebuild::render_soul_md(paths, &derived, &format!("render {t_ref}"))?;

    // 대기본은 여기서 사라진다. 승인된 내용은 이미 `SOUL.md`에 있다.
    remove_next(app)?;
    Ok(id)
}

/// 거절. **관측을 기록하지 않는다.** `SOUL.next.md`를 지우고 트레이스에만 남긴다.
pub fn reject(app: &crate::App) -> Result<()> {
    remove_next(app)?;
    // §11.3 — 트레이스는 재현성의 일부가 아니다. 실패해도 거절 자체는 성공이다.
    if let Some(tracer) = &app.tracer {
        let _ = tracer.write(&TraceEntry {
            ts: Ts::now(),
            purpose: "reflect".to_string(),
            model: app.config.models.reflect.clone(),
            prompt_sha256: None,
            tokens_in: 0,
            tokens_out: 0,
            cost_usd_est: 0.0,
            latency_ms: 0,
            response_raw: None,
            error: Some("사용자가 성찰 제안을 거절했습니다 (§11.2)".to_string()),
        });
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────── 내부

/// 대기본을 지운다. 없으면 아무 일도 아니다.
fn remove_next(app: &crate::App) -> Result<()> {
    match std::fs::remove_file(app.paths.soul_next_md()) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// 제안을 반영한 `SOUL.md` 전문 (§13 화면 4의 오른쪽).
///
/// 아직 관측이 아니므로 **디스크의 어떤 상태도 바꾸지 않는다.** 승인 후 실제로 렌더되는
/// 문서와 같은 렌더러를 쓰되, 입력만 제안값으로 바꿔 끼운다.
fn render_next(derived: &Derived, delta: &SoulDelta, current_md: &str) -> Result<String> {
    // 파싱 실패는 그대로 올린다 — §15는 "파일을 쓰지 않고 알린다"이다.
    let doc = if current_md.trim().is_empty() {
        None
    } else {
        Some(soulmd::parse(current_md)?)
    };
    // §18-5 — 사람이 쓴 블록은 미리보기에서도 그대로 이월한다.
    let human = doc
        .as_ref()
        .and_then(|d| d.human_body())
        .unwrap_or("")
        .to_string();
    let current_profile = doc.as_ref().and_then(|d| d.block(PROFILE_BLOCK));
    let current_text = current_profile
        .map(|b| b.normalized_body())
        .unwrap_or_default();
    let current_rev = current_profile.and_then(|b| b.rev).unwrap_or(0);

    // §8.3 규칙 4 — `rev`는 블록을 실제로 건드린 제안일 때만 오른다.
    let (profile_text, profile_rev) = match delta.blocks.get(PROFILE_BLOCK) {
        Some(b) => (b.to_text.clone(), current_rev.saturating_add(1)),
        None => (current_text, current_rev),
    };

    let preview = with_axis_delta(derived, &delta.axis_delta);
    let examples: Vec<ObsId> = preview
        .coherence_sensory
        .as_ref()
        .map(|c| c.examples.clone())
        .unwrap_or_default();

    Ok(soulmd::render(&RenderInput {
        derived: &preview,
        profile_text: &profile_text,
        profile_rev,
        human_text: &human,
        divergence_examples: &examples,
    }))
}

/// §12.1 — `axis_delta`는 `offset`에 더해지고 `final = clamp(computed + offset, 0, 1)`이다.
///
/// 승인 전에는 관측이 없으므로 파생값을 다시 계산할 수 없다. 미리보기에서는
/// **축만** 제안값으로 갈아 끼운다. 다른 지표는 관측이 늘어야 움직인다.
fn with_axis_delta(derived: &Derived, axis_delta: &AxisDelta) -> Derived {
    let mut out = derived.clone();
    let mut offset = out.axes_offset;
    for (name, d) in axis_delta {
        // 알 수 없는 축은 가드레일이 이미 거른다 (§11.2). 여기서는 조용히 무시한다.
        if let Some(a) = Axis::parse(name) {
            offset.set(a, offset.get(a) + d);
        }
    }
    out.axes_offset = offset;
    out.axes_final = out
        .axes_computed
        .map(|c| soul_core::derived::axes::finalize(c, offset));
    out
}

/// "수정 후 승인" — `blocks.profile.to_text`를 사용자가 고친 텍스트로 바꾼다 (§13 화면 4).
///
/// `text`는 `profile` 블록 본문이거나, 그 블록을 담은 문서다. 어느 쪽이든 기록되는 것은
/// `profile` 본문뿐이다 — `soul:gen`은 파생값이고 `soul:human`은 원격으로 나가지 않는
/// 사람의 글이므로(§D4) 어느 쪽도 `soul_delta`에 들어가지 않는다.
fn with_modified_profile(
    delta: &SoulDelta,
    paths: &soul_core::paths::Paths,
    text: &str,
) -> Result<SoulDelta> {
    let to_text = profile_text_from(text)?;
    let from_hash = match delta.blocks.get(PROFILE_BLOCK) {
        Some(b) => b.from_hash.clone(),
        // 축만 제안한 델타에 사람이 문장을 얹은 경우. 현재 파일의 실제 해시를 쓴다.
        None => current_profile_hash(paths)?,
    };
    let mut out = delta.clone();
    out.blocks
        .insert(PROFILE_BLOCK.to_string(), BlockDelta { from_hash, to_text });
    Ok(out)
}

/// 사용자가 고친 텍스트에서 **`profile` 블록 본문만** 뽑는다 (§D4·§18-4·T29).
///
/// ## 전문 폴백을 되살리지 말 것
///
/// 예전에는 파싱에 실패하거나 `profile` 블록이 없으면 **받은 텍스트 전체**를 본문으로
/// 삼았다. 관대해 보이지만 그것이 사람이 쓴 글의 유출 경로였다: 화면 4의 편집 상자가
/// 문서 전문을 주므로, 사용자가 `profile` 블록을 지우거나 마커 한 줄을 깨뜨린 채
/// 승인하면 `soul:human` 본문이 그대로 `soul_delta.blocks.profile.to_text`에 들어간다.
/// `profile`은 `soul:neg`이라 **다음 성찰 호출부터 원격 모델에게 전송된다.**
/// 나간 뒤에는 되돌릴 방법이 없다.
///
/// 그래서 여기서는 "모르겠으면 전부 쓴다"가 아니라 "모르겠으면 쓰지 않고 알린다"다 (§15).
fn profile_text_from(text: &str) -> Result<String> {
    // 파싱 실패는 그대로 올린다. 깨진 마커 안쪽에 무엇이 있는지 우리는 모른다 (§8.3 규칙 6).
    let doc = soulmd::parse(text)?;
    if let Some(b) = doc.block(PROFILE_BLOCK) {
        return Ok(b.normalized_body());
    }
    if !doc.blocks.is_empty() {
        // 문서로는 파싱되는데 `profile`이 없다 — 사용자가 블록을 지웠거나 id를 바꿨다.
        // 다른 블록으로 대신하지 않는다. 조용히 넘어가지도 않는다 (§15).
        return Err(SoulError::invalid(format!(
            "수정한 문서에 `{PROFILE_BLOCK}` 블록이 없습니다. \
             `<!-- soul:neg id={PROFILE_BLOCK} … -->` 블록의 본문만 고쳐 주세요 (§8.3)."
        )));
    }

    // 마커가 하나도 없는 순수 문단 — 편집 상자가 `profile` 본문만 줄 때의 정상 경로다.
    // 그래도 `soul:human` 마커 문자열이 보이면 거부한다: 마커처럼 생겼지만 파서가 마커로
    // 인정하지 않는 줄(예 `<!-- soul:human --> 메모`)은 산문으로 흘러들고, 그 아래 붙은
    // 사람의 글이 여기를 통과하면 원격으로 나간다 (§D4).
    if text.contains(HUMAN_MARKER) {
        return Err(SoulError::invalid(format!(
            "수정한 텍스트에 `{HUMAN_MARKER}` 마커가 있습니다. \
             사람이 쓴 블록은 관측에 기록하지 않습니다 — `{PROFILE_BLOCK}` 본문만 넣어 주세요 (§D4)."
        )));
    }

    // §8.3 규칙 3의 정규화는 줄 **뒤쪽** 공백만 없애므로, 편집 상자에서 딸려 오는
    // 앞쪽 여백은 여기서 한 번 털어 낸다.
    Ok(soulmd::normalize_for_hash(text.trim()))
}

/// 화면 4의 편집 상자에 실릴 `profile` 본문 두 개 (현재 / 제안).
///
/// 좌우 diff **표시**는 전문(`current_md` · `next_md`)을 쓰지만, **편집 대상**은 이 값이다.
/// 전문을 편집시키면 사용자가 고친 `soul:human`이 승인 경로로 되돌아온다 (§D4).
fn profile_texts(delta: &SoulDelta, current_md: &str) -> Result<(String, String)> {
    let current = if current_md.trim().is_empty() {
        // 아직 `SOUL.md`가 없다. 첫 성찰에서만 나오는 경우다 (§13 화면 4).
        String::new()
    } else {
        soulmd::parse(current_md)?
            .block(PROFILE_BLOCK)
            .map(|b| b.normalized_body())
            .unwrap_or_default()
    };
    let proposed = match delta.blocks.get(PROFILE_BLOCK) {
        Some(b) => b.to_text.clone(),
        // 축만 바꾸는 제안. 편집의 출발점은 지금의 본문이다 (§10.5).
        None => current.clone(),
    };
    Ok((current, proposed))
}

/// §11.2 가드레일 — `profile` 본문은 비어 있지 않은 한국어 3~6문장이다.
///
/// 제안 때 `soul_agent::reflect::check_guardrails`가 이미 보는 규칙인데 승인 때 **다시**
/// 보는 이유: "수정 후 승인"의 본문은 모델이 아니라 사람이 쓴 것이라 그 검사를 한 번도
/// 거치지 않는다. 여기서 통과시키면 §11.2는 우회 가능한 장식이 된다.
///
/// 문장 수와 한국어 판정은 제안 때와 **같은 함수**를 부른다. 여기서 따로 세면 두 경로가
/// 서로 다른 "3~6문장"을 갖게 되고, 화면 4에서만 통과하는 텍스트가 생긴다 (§R2).
fn validate_profile_text(text: &str) -> Result<()> {
    let t = text.trim();
    if t.is_empty() {
        return Err(SoulError::guardrail(
            "profile 본문이 비어 있습니다. 내용을 지우려면 승인이 아니라 거절입니다 (§11.2).",
        ));
    }
    let n = soul_agent::schemas::count_sentences(t);
    if !(3..=6).contains(&n) {
        return Err(SoulError::guardrail(format!(
            "profile 본문이 {n}문장입니다. 3~6문장이어야 합니다 (§11.2)."
        )));
    }
    if !soul_agent::reflect::has_hangul(t) {
        return Err(SoulError::guardrail(
            "profile 본문에 한국어가 없습니다. 한국어로 써 주세요 (§11.2).",
        ));
    }
    Ok(())
}

fn current_profile_hash(paths: &soul_core::paths::Paths) -> Result<String> {
    let Ok(text) = std::fs::read_to_string(paths.soul_md()) else {
        return Ok(soulmd::block_hash(""));
    };
    let doc = soulmd::parse(&text)?;
    Ok(doc
        .block(PROFILE_BLOCK)
        .map(|b| b.actual_hash())
        .unwrap_or_else(|| soulmd::block_hash("")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::testkit::{record_text, temp_app, TempApp};
    use soul_core::obs::{ModelRef, Window};
    use soul_core::SCHEMA_VERSION;
    use std::collections::BTreeMap;

    // ─────────────────────────────────────────────────────────── 픽스처

    /// §11.2 가드레일을 만족하는 `profile` 본문 (3~6문장·한국어).
    ///
    /// 승인 경로가 이 규칙을 다시 검사하므로(§11.2가 장식이 되지 않도록), 픽스처도
    /// 실제 제안이 통과해야 하는 모양이어야 한다.
    const OK_PROFILE: &str = "인공물보다 방치된 것을 고른다. \
                              사람이 방금 지나간 자리를 오래 본다. \
                              정오보다 해질 무렵의 빛을 택한다.";

    fn model_ref() -> ModelRef {
        ModelRef {
            provider: "openai".into(),
            id: "gpt-x".into(),
            prompt_sha256: None,
            calls: vec![],
        }
    }

    /// `profile` 블록과 축 하나를 건드리는 제안.
    fn proposal(app: &crate::App, to_text: &str) -> Proposal {
        let (id, ts, schema) = soul_core::obs::new_header();
        let ids = app.store.ids().unwrap();
        let window = Window {
            from: ids.first().cloned().unwrap_or_else(|| id.clone()),
            to: ids.last().cloned().unwrap_or_else(|| id.clone()),
        };
        let mut blocks = BTreeMap::new();
        blocks.insert(
            PROFILE_BLOCK.to_string(),
            BlockDelta {
                from_hash: current_profile_hash(&app.paths).unwrap(),
                to_text: to_text.to_string(),
            },
        );
        let mut axis_delta = AxisDelta::new();
        axis_delta.insert("grain".to_string(), 0.05);

        let delta = SoulDelta {
            id,
            ts,
            schema,
            window,
            blocks,
            axis_delta,
            morphology_delta: None,
            cites: ids.into_iter().take(3).collect(),
            rationale: "other_reason 셀이 늘었다".into(),
            model: model_ref(),
        };
        let current_md = std::fs::read_to_string(app.paths.soul_md()).unwrap_or_default();
        let next_md = render_next(&Derived::default(), &delta, &current_md).unwrap();
        let (current_profile_text, proposed_profile_text) =
            profile_texts(&delta, &current_md).unwrap();
        Proposal {
            delta,
            next_md,
            current_md,
            current_profile_text,
            proposed_profile_text,
        }
    }

    /// 사람이 쓴 블록이 들어 있는 `SOUL.md`를 만들어 둔다.
    fn seed_soul_md(app: &crate::App, human: &str) {
        let text = soulmd::render(&RenderInput {
            derived: &Derived::default(),
            profile_text: "인공물보다 방치된 것을 고른다.",
            profile_rev: 3,
            human_text: human,
            divergence_examples: &[],
        });
        std::fs::write(app.paths.soul_md(), text.as_bytes()).unwrap();
        soul_core::git::commit_paths(
            &app.paths.soul(),
            &[std::path::Path::new("SOUL.md")],
            "render seed",
        )
        .unwrap();
    }

    fn commit_count(app: &crate::App) -> usize {
        soul_core::git::log_messages(&app.paths.soul(), 50)
            .unwrap()
            .len()
    }

    fn fixture(tag: &str) -> TempApp {
        let fx = temp_app(tag);
        record_text(&fx.app, "차갑고 정돈된, 사람이 방금 지워진 실내", None);
        record_text(&fx.app, "젖은 아스팔트 위의 신호등", None);
        record_text(&fx.app, "빈 주차장의 정오", None);
        // 작업 트리를 깨끗한 상태에서 시작한다 — 그래야 T20의 `is_clean`이 의미를 갖는다.
        soul_core::git::commit_all(&fx.app.paths.soul(), "fixture").unwrap();
        fx
    }

    // ───────────────────────────────────────────── reject — 관측 0건

    /// 거절은 **관측을 만들지 않는다.** 대기본만 사라진다 (§11.2).
    #[test]
    fn reject_records_no_observation_and_no_commit() {
        let fx = fixture("reflect-reject");
        let app = &fx.app;
        std::fs::write(app.paths.soul_next_md(), "# SOUL\n제안본\n").unwrap();
        let before_obs = app.store.count().unwrap();
        let before_commits = commit_count(app);
        // T20 — 대기본은 `.gitignore`에 있으므로 `git status`에 나타나지 않는다.
        assert!(soul_core::git::is_clean(&app.paths.soul()).unwrap());

        reject(app).unwrap();

        assert_eq!(
            app.store.count().unwrap(),
            before_obs,
            "관측이 늘면 안 된다"
        );
        assert_eq!(commit_count(app), before_commits, "커밋도 늘면 안 된다");
        assert!(!app.paths.soul_next_md().exists());
        assert!(soul_core::git::is_clean(&app.paths.soul()).unwrap());
    }

    /// 대기본이 없어도 거절은 실패하지 않는다 (앱을 껐다 켠 뒤 거절하는 경우).
    #[test]
    fn reject_without_a_pending_file_is_ok() {
        let fx = fixture("reflect-reject-none");
        assert!(!fx.app.paths.soul_next_md().exists());
        reject(&fx.app).unwrap();
    }

    // ───────────────────────────────────────────── approve — 커밋 2개

    /// 승인 한 번에 커밋 둘: `soul_delta <ULID>` + `render <T_ref>` (§R8).
    #[tokio::test]
    async fn approve_makes_exactly_two_commits() {
        let fx = fixture("reflect-approve");
        let app = &fx.app;
        seed_soul_md(app, "습도라는 말을 쓰기 시작한 건 3월부터다.");
        // §11.2 — 승인 경로도 3~6문장·한국어를 본다. 픽스처도 그 규칙을 지킨다.
        let p = proposal(
            app,
            "인공물보다 방치된 것을 고른다. 사람의 흔적이 남은 쪽을 택한다. 정오보다 해질 무렵을 본다.",
        );
        std::fs::write(app.paths.soul_next_md(), &p.next_md).unwrap();

        let before = commit_count(app);
        let id = approve(app, &p, None).await.unwrap();

        let log = soul_core::git::log_messages(&app.paths.soul(), 50).unwrap();
        assert_eq!(log.len(), before + 2, "soul_delta + render = 2개: {log:?}");
        assert!(log[0].starts_with("render "), "{log:?}");
        assert_eq!(log[1], format!("soul_delta {id}"));

        // 관측이 실제로 기록되었고 내용이 제안과 같다.
        let obs = app.store.read(&id).unwrap();
        let d = obs.as_soul_delta().expect("soul_delta");
        assert_eq!(
            d.blocks[PROFILE_BLOCK].to_text,
            "인공물보다 방치된 것을 고른다. 사람의 흔적이 남은 쪽을 택한다. 정오보다 해질 무렵을 본다."
        );

        // 재렌더 결과에 제안 문장이 들어 있다.
        let md = std::fs::read_to_string(app.paths.soul_md()).unwrap();
        assert!(md.contains("사람의 흔적이 남은 쪽을 택한다"), "{md}");
        // §18-5 · T4 — `soul:human`은 재렌더에서도 살아남는다.
        assert!(
            md.contains("습도라는 말을 쓰기 시작한 건 3월부터다."),
            "{md}"
        );
        // 대기본은 사라진다.
        assert!(!app.paths.soul_next_md().exists());
        assert!(soul_core::git::is_clean(&app.paths.soul()).unwrap(), "T20");
    }

    /// "수정 후 승인" — 기록되는 것은 **수정된 텍스트**다 (§13 화면 4).
    #[tokio::test]
    async fn approve_with_modified_text_records_the_edit() {
        const EDIT: &str = "사람이 고쳐 쓴 문장이다. 두 번째 문장을 덧붙였다. 세 번째로 끝낸다.";
        let fx = fixture("reflect-modified");
        let app = &fx.app;
        seed_soul_md(app, "");
        let p = proposal(app, OK_PROFILE);

        let id = approve(app, &p, Some(EDIT)).await.unwrap();

        let obs = app.store.read(&id).unwrap();
        let d = obs.as_soul_delta().unwrap();
        assert_eq!(d.blocks[PROFILE_BLOCK].to_text, EDIT);
        assert_eq!(
            d.blocks[PROFILE_BLOCK].from_hash, p.delta.blocks[PROFILE_BLOCK].from_hash,
            "from_hash는 제안 시점 것을 유지한다 (§11.2 가드레일)"
        );
        assert_eq!(d.axis_delta, p.delta.axis_delta, "축 제안은 그대로다");

        let md = std::fs::read_to_string(app.paths.soul_md()).unwrap();
        assert!(md.contains("사람이 고쳐 쓴 문장이다."), "{md}");
        assert!(!md.contains("사람이 방금 지나간 자리를 오래 본다."), "{md}");
    }

    /// 화면 4가 문서 전문을 돌려줘도 `profile` 블록만 기록한다 (§D4).
    #[tokio::test]
    async fn modified_full_document_keeps_only_the_profile_block() {
        const EDIT: &str = "전문 편집으로 들어온 문장이다. 두 번째 문장이다. 세 번째 문장이다.";
        let fx = fixture("reflect-fulldoc");
        let app = &fx.app;
        seed_soul_md(app, "내가 쓴 줄");
        let p = proposal(app, OK_PROFILE);

        let edited = soulmd::render(&RenderInput {
            derived: &Derived::default(),
            profile_text: EDIT,
            profile_rev: 4,
            human_text: "사람 블록은 델타에 들어가지 않는다",
            divergence_examples: &[],
        });
        let id = approve(app, &p, Some(&edited)).await.unwrap();

        let obs = app.store.read(&id).unwrap();
        let d = obs.as_soul_delta().unwrap();
        assert_eq!(d.blocks[PROFILE_BLOCK].to_text, EDIT);
        assert_eq!(d.blocks.len(), 1, "profile 외의 블록을 만들지 않는다");
        assert!(
            !d.blocks[PROFILE_BLOCK].to_text.contains("사람 블록은"),
            "§D4 — `soul:human`은 델타에 들어가지 않는다"
        );
    }

    /// §18-4 · §D4 · T29 — "수정 후 승인"이 `soul:human`을 원격으로 새게 하면 안 된다.
    ///
    /// 편집 상자가 문서 **전문**을 주므로 사용자가 `profile` 블록을 통째로 지운 전문을
    /// 돌려줄 수 있다. 그 전문을 `to_text`로 삼으면 `soul:human` 본문이
    /// `soul_delta.blocks.profile.to_text`에 들어가고, `profile`은 `soul:neg`이므로
    /// **다음 성찰 호출부터 원격 모델에게 전송된다.** 한 번 나가면 되돌릴 수 없다.
    #[tokio::test]
    async fn modified_text_never_carries_soul_human_into_the_delta() {
        const SECRET: &str = "아무한테도-안-보여줄-문장-Q7X2K9";
        let fx = fixture("reflect-human-leak");
        let app = &fx.app;
        seed_soul_md(app, SECRET);
        let p = proposal(app, OK_PROFILE);

        // 사용자가 편집 상자에서 `profile` 블록만 지운 전문. 문서로는 멀쩡히 파싱된다.
        let edited = format!(
            "# SOUL\n\n<!-- soul:gen id=header -->\n갱신 2026-08-13\n<!-- /soul:gen -->\n\n\
             <!-- soul:human -->\n{SECRET}\n<!-- /soul:human -->\n"
        );

        let result = approve(app, &p, Some(&edited)).await;

        // 기록된 것이 있다면, 그 안에 사람이 쓴 글이 있어서는 안 된다.
        let set = app.store.load_set().unwrap();
        for d in set.soul_deltas() {
            for (bid, b) in &d.blocks {
                assert!(
                    !b.to_text.contains(SECRET),
                    "soul:human이 `{bid}` 블록으로 샜다 (§D4): {}",
                    b.to_text
                );
            }
        }
        // §15 — 조용히 다른 것을 기록하지 말고 알린다.
        let err = result.expect_err("profile 블록이 없는 전문은 거부되어야 한다");
        assert!(err.to_string().contains("profile"), "{err}");

        // 재렌더도 일어나지 않았다 — `SOUL.md`의 profile은 그대로다.
        let md = std::fs::read_to_string(app.paths.soul_md()).unwrap();
        let doc = soulmd::parse(&md).unwrap();
        assert!(
            !doc.block(PROFILE_BLOCK).unwrap().body.contains(SECRET),
            "{md}"
        );
    }

    /// 승인은 `axis_delta`를 파생 `offset`에 반영한다 (§12.1).
    #[tokio::test]
    async fn approve_moves_the_axis_offset() {
        let fx = fixture("reflect-offset");
        let app = &fx.app;
        seed_soul_md(app, "");
        let p = proposal(app, OK_PROFILE);

        assert_eq!(app.derived().unwrap().axes_offset.grain, 0.0);
        approve(app, &p, None).await.unwrap();
        let after = app.derived().unwrap();
        assert!(
            (after.axes_offset.grain - 0.05).abs() < 1e-9,
            "{:?}",
            after.axes_offset
        );
    }

    // ───────────────────────────────────────────── propose — 트리거 판정

    /// 임계치에 못 미치면 에이전트를 부르지 않는다 (§11.2).
    ///
    /// (키가 없어 `openai()`가 실패하므로, 이 테스트가 통과한다는 것 자체가
    ///  네트워크 경로에 들어가지 않았다는 증거다.)
    #[tokio::test]
    async fn propose_returns_none_below_the_trigger() {
        let fx = fixture("reflect-trigger");
        let app = &fx.app;
        assert!(app.config.thresholds.reflect_trigger_ingests > 3);
        assert!(propose(app, false).await.unwrap().is_none());
        assert!(!app.paths.soul_next_md().exists());
    }

    // ───────────────────────────────────────────── 미리보기 렌더

    #[test]
    fn preview_carries_over_human_and_bumps_rev() {
        let fx = fixture("reflect-preview");
        let app = &fx.app;
        seed_soul_md(app, "사람이 쓴 줄");
        let current = std::fs::read_to_string(app.paths.soul_md()).unwrap();
        let p = proposal(app, "제안된 문장.");

        let next = render_next(&Derived::default(), &p.delta, &current).unwrap();
        assert!(next.contains("제안된 문장."), "{next}");
        assert!(
            next.contains("사람이 쓴 줄"),
            "§18-5 — 사람 블록 이월: {next}"
        );
        assert!(next.contains("rev=4"), "rev +1 (§8.3 규칙 4): {next}");
        assert_ne!(next, current);
    }

    #[test]
    fn preview_without_a_profile_block_keeps_rev() {
        let mut delta_blocks = BTreeMap::new();
        delta_blocks.insert(
            "other".to_string(),
            BlockDelta {
                from_hash: "aaa".into(),
                to_text: "다른 블록".into(),
            },
        );
        let (id, ts, schema) = soul_core::obs::new_header();
        let delta = SoulDelta {
            id: id.clone(),
            ts,
            schema,
            window: Window {
                from: id.clone(),
                to: id,
            },
            blocks: delta_blocks,
            axis_delta: AxisDelta::new(),
            morphology_delta: None,
            cites: vec![],
            rationale: "축만".into(),
            model: model_ref(),
        };
        let current = soulmd::render(&RenderInput {
            derived: &Derived::default(),
            profile_text: "그대로인 문장.",
            profile_rev: 7,
            human_text: "",
            divergence_examples: &[],
        });
        let next = render_next(&Derived::default(), &delta, &current).unwrap();
        assert!(
            next.contains("rev=7"),
            "건드리지 않은 블록의 rev는 그대로다"
        );
        assert!(next.contains("그대로인 문장."));
    }

    /// 축 제안은 `final = clamp(computed + offset, 0, 1)`로 미리 보인다 (§12.1, T15).
    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn preview_applies_axis_delta_to_the_offset() {
        let mut derived = Derived::default();
        derived.axes_computed = Some(soul_core::obs::Axes::from_array([0.98; 8]));
        derived.axes_final = Some(soul_core::obs::Axes::from_array([0.98; 8]));

        let mut axis_delta = AxisDelta::new();
        axis_delta.insert("grain".to_string(), 0.10);
        axis_delta.insert("없는축".to_string(), 0.10);

        let preview = with_axis_delta(&derived, &axis_delta);
        assert!((preview.axes_offset.grain - 0.10).abs() < 1e-9);
        assert_eq!(
            preview.axes_final.unwrap().grain,
            1.0,
            "clamp 되어야 한다 (T15)"
        );
        assert_eq!(preview.axes_final.unwrap().chroma, 0.98, "다른 축은 그대로");
        assert_eq!(preview.axes_offset.chroma, 0.0, "알 수 없는 축은 무시한다");
    }

    // ─────────────────────────────────────── 수정 텍스트 해석 (§D4·§18-4)

    /// 마커가 하나도 없는 문단은 그대로 `profile` 본문이다 — 편집 상자의 정상 경로다.
    #[test]
    fn modified_text_accepts_a_bare_paragraph() {
        assert_eq!(profile_text_from("  한 문장.  ").unwrap(), "한 문장.");
    }

    /// 문서로 파싱되는데 `profile` 블록이 없으면 **에러**다. 전문으로 대신하지 않는다 (§15).
    #[test]
    fn a_document_without_a_profile_block_is_an_error() {
        let doc = "# SOUL\n\n<!-- soul:gen id=header -->\n갱신\n<!-- /soul:gen -->\n";
        let err = profile_text_from(doc).expect_err("전문 폴백이 되살아났다");
        assert!(err.to_string().contains("profile"), "{err}");
    }

    /// 파싱되지 않는 텍스트도 에러다. 예전에는 이것이 **전문 폴백의 주된 입구**였다:
    /// 마커 한 줄이 깨진 전문이 통째로 `to_text`가 되어 `soul:human`을 실어 날랐다.
    #[test]
    fn unparsable_modified_text_is_an_error_not_a_fallback() {
        // 닫는 마커가 없다 (§8.3 규칙 6).
        let broken = "<!-- soul:human -->\n아무한테도 안 보여줄 문장.\n";
        let err = profile_text_from(broken).expect_err("파싱 실패가 전문 폴백으로 새면 안 된다");
        assert!(
            !err.to_string().contains("아무한테도"),
            "에러 메시지에도 싣지 않는다: {err}"
        );
    }

    /// 마커처럼 생겼지만 파서가 산문으로 흘려보내는 줄. 순수 문단 경로의 마지막 방어선이다.
    #[test]
    fn a_bare_paragraph_carrying_a_human_marker_is_rejected() {
        // 뒤에 글자가 붙어 있어 마커로 인정되지 않는다 → 파싱은 성공하고 블록은 0개다.
        let text = "<!-- soul:human --> 라고 적어 둔다\n아무한테도 안 보여줄 문장.";
        assert!(soulmd::parse(text).unwrap().blocks.is_empty(), "전제 확인");
        let err = profile_text_from(text).expect_err("soul:human 마커는 거부한다 (§D4)");
        assert!(err.to_string().contains(HUMAN_MARKER), "{err}");
    }

    /// 전문에 `profile`이 있으면 그 본문만 나온다 — 나머지 블록은 쳐다보지 않는다.
    #[test]
    fn a_full_document_yields_only_the_profile_body() {
        let doc = soulmd::render(&RenderInput {
            derived: &Derived::default(),
            profile_text: "제자리에 있는 문장.",
            profile_rev: 2,
            human_text: "사람이 쓴 줄",
            divergence_examples: &[],
        });
        assert_eq!(profile_text_from(&doc).unwrap(), "제자리에 있는 문장.");
    }

    // ─────────────────────────────────── 승인 경로의 §11.2 가드레일

    /// §11.2는 승인 때도 걸린다. 제안 때만 보면 "수정 후 승인"이 우회로가 된다.
    #[tokio::test]
    async fn approve_rechecks_the_profile_guardrail_on_the_edited_text() {
        let fx = fixture("reflect-guardrail");
        let app = &fx.app;
        seed_soul_md(app, "");
        let p = proposal(app, OK_PROFILE);
        let before = app.store.count().unwrap();

        // 1문장 — 3~6문장 규칙 위반.
        let err = approve(app, &p, Some("한 문장뿐이다."))
            .await
            .expect_err("문장 수를 보지 않았다");
        assert!(err.to_string().contains("문장"), "{err}");

        // 한국어가 없다.
        let err = approve(app, &p, Some("One. Two. Three."))
            .await
            .expect_err("한국어 검사를 하지 않았다");
        assert!(err.to_string().contains("한국어"), "{err}");

        // 비어 있다.
        assert!(approve(app, &p, Some("   ")).await.is_err());

        // 거부된 승인은 아무것도 기록하지 않는다 (§15).
        assert_eq!(app.store.count().unwrap(), before, "관측이 늘면 안 된다");
        assert!(soul_core::git::is_clean(&app.paths.soul()).unwrap(), "T20");
    }

    // ─────────────────────────────────── 편집 상자에 실리는 값 (§D4)

    /// `Proposal`은 편집용으로 **전문이 아니라 `profile` 본문**을 들고 있어야 한다.
    /// 전문을 편집시키면 사용자가 고친 `soul:human`이 승인 경로로 되돌아온다 (§18-4).
    #[test]
    fn proposal_exposes_the_profile_text_without_the_human_block() {
        const SECRET: &str = "사람이-쓴-줄-K3M8";
        let fx = fixture("reflect-editbox");
        let app = &fx.app;
        seed_soul_md(app, SECRET);
        let p = proposal(app, OK_PROFILE);

        assert_eq!(p.proposed_profile_text, OK_PROFILE);
        assert_eq!(p.current_profile_text, "인공물보다 방치된 것을 고른다.");
        assert!(!p.current_profile_text.contains(SECRET));
        assert!(!p.proposed_profile_text.contains(SECRET));
        // 표시용 전문에는 그대로 있다 — 목적지가 로컬 화면이라 §D4 대상이 아니다.
        assert!(
            p.current_md.contains(SECRET),
            "좌우 diff 표시는 전문 그대로다"
        );
    }

    /// 축만 바꾸는 제안이면 편집의 출발점은 **지금의** `profile` 본문이다 (§10.5).
    #[test]
    fn profile_texts_fall_back_to_the_current_body_when_the_block_is_untouched() {
        let current = soulmd::render(&RenderInput {
            derived: &Derived::default(),
            profile_text: "그대로인 문장.",
            profile_rev: 7,
            human_text: "사람이 쓴 줄",
            divergence_examples: &[],
        });
        let (id, ts, schema) = soul_core::obs::new_header();
        let delta = SoulDelta {
            id: id.clone(),
            ts,
            schema,
            window: Window {
                from: id.clone(),
                to: id,
            },
            blocks: BTreeMap::new(),
            axis_delta: AxisDelta::new(),
            morphology_delta: None,
            cites: vec![],
            rationale: "축만".into(),
            model: model_ref(),
        };
        let (cur, next) = profile_texts(&delta, &current).unwrap();
        assert_eq!(cur, "그대로인 문장.");
        assert_eq!(next, "그대로인 문장.");
        assert!(!next.contains("사람이 쓴 줄"), "§D4");
    }

    #[test]
    fn schema_version_is_carried_from_the_proposal() {
        let fx = fixture("reflect-schema");
        let p = proposal(&fx.app, OK_PROFILE);
        assert_eq!(p.delta.schema, SCHEMA_VERSION);
    }
}
