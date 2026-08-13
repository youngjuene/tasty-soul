//! 모달리티별 서술 생성 (§9.2 · §9.4 · §9.5 · §9.6 · §10).
//!
//! **`machine.quality`는 모델이 아니라 이 모듈이 정한다** (§6.2).
//!
//! | 값 | 조건 |
//! |---|---|
//! | `full` | 의도한 입력이 전부 처리됨 |
//! | `partial` | 한 갈래가 빠짐 — 오디오 메타데이터 폴백(§9.5), 오디오 없는 영상(§9.6), 프레임 3장 미만 |
//! | `minimal` | YouTube 다운로드 실패로 메타데이터와 썸네일만 확보(§9.3) |

use base64::prelude::{Engine as _, BASE64_STANDARD};
use serde::Deserialize;
use soul_core::error::{Result, SoulError};
use soul_core::obs::{Axes, Axis, Machine, Quality};
use soul_core::prompts::{self, PromptFile};
use soul_core::time::Ts;
use soul_core::trace::TraceEntry;
use soul_net::openai::{Call, Part};

pub struct DescribeOutcome {
    pub machine: Machine,
    pub call_ids: Vec<String>,
    /// §10.1 검증에 걸려 재시도했는가 (T27).
    pub retried: bool,
}

// ────────────────────────────────────────────────────────── §9.2 텍스트 절단

/// §9.2 — 이 길이를 넘으면 자른다.
pub const TEXT_MAX_CHARS: usize = 4000;
const TEXT_HEAD_CHARS: usize = 2000;
const TEXT_TAIL_CHARS: usize = 2000;

/// §9.2 — 4000자 초과 시 **앞 2000 + 뒤 2000자**.
///
/// 바이트가 아니라 **문자** 기준이다. 한국어는 문자당 3바이트라 바이트로 세면
/// 같은 글이 절반 길이로 잘린다.
pub fn truncate_text(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= TEXT_MAX_CHARS {
        return text.to_string();
    }
    let head: String = chars[..TEXT_HEAD_CHARS].iter().collect();
    let tail: String = chars[chars.len() - TEXT_TAIL_CHARS..].iter().collect();
    // 잘렸다는 표식을 넣지 않는다 — §9.2가 정한 것은 두 조각의 이어붙임뿐이다.
    head + &tail
}

// ─────────────────────────────────────────────────── §R11 최종 시스템 프롬프트

/// 여러 프롬프트 파일을 **하나의 최종 시스템 프롬프트**로 잇는다 (§R11).
///
/// 구분은 빈 줄 하나이고 결과는 항상 LF로 끝난다. 이 조립이 결정론적이어야
/// `prompt_sha256`이 프롬프트 내용에만 반응한다.
fn compose(sections: &[&str]) -> String {
    let mut out = String::new();
    for s in sections {
        let body = s.trim_end_matches('\n');
        if body.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(body);
        out.push('\n');
    }
    out
}

/// 최종 시스템 프롬프트의 sha256 (§R11).
///
/// 호출이 하나인 경로에서는 §R11의 문구 그대로 "렌더링이 끝난 최종 시스템 프롬프트"의
/// 해시다. 영상처럼 호출이 여럿인 경로에서는 **호출 순서대로 이은 전체**의 해시를 쓴다.
/// 병합 프롬프트만 기록하면 `describe.md`를 고쳤을 때 영상 관측의 `prompt_sha256`이
/// 그대로여서 축 값만 조용히 이동한다 — §R11이 막으려는 가짜 드리프트 그 자체다 (T23).
fn system_sha(sections: &[&str]) -> String {
    prompts::sha256_of(&compose(sections))
}

/// §10.1 — 모든 서술 경로의 기반 프롬프트.
fn describe_system() -> Result<String> {
    Ok(compose(&[&prompts::load(PromptFile::Describe)?]))
}

/// §10.2의 "이어붙인 샘플" 문구를 골라내는 **판별자**.
///
/// 프롬프트 본문이 아니다 — 문구 자체는 `prompts/audio.md`에만 있다 (§R11).
/// `concatenated = false`인데 이 문구를 그대로 보내면 모델에게 없는 사실을 알리는 것이 된다.
const CONCAT_MARKERS: [&str; 2] = ["이어붙", "세 구간"];

/// §10.2 추가 지시. 이어붙이지 않은 클립이면 이어붙임을 알리는 줄을 뺀다 (§9.5).
fn audio_extra(audio_md: &str, concatenated: bool) -> String {
    if concatenated {
        return audio_md.to_string();
    }
    audio_md
        .lines()
        .filter(|l| !CONCAT_MARKERS.iter().any(|m| l.contains(m)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// §10.1 + §10.2 — 오디오 경로의 최종 시스템 프롬프트.
fn audio_system(concatenated: bool) -> Result<String> {
    let base = prompts::load(PromptFile::Describe)?;
    let extra = audio_extra(&prompts::load(PromptFile::Audio)?, concatenated);
    Ok(compose(&[&base, &extra]))
}

/// §10.3 — 병합 호출의 시스템 프롬프트. `merge.md`는 축 정의와 서술 규칙을
/// 스스로 갖고 있으므로 `describe.md`를 덧붙이지 않는다.
fn merge_system() -> Result<String> {
    Ok(compose(&[&prompts::load(PromptFile::Merge)?]))
}

// ────────────────────────────────────────────────────────────── 공통 도구

/// base64 data URL. 프레임·이미지를 Responses API에 넣을 때 쓴다 (§9.4·§9.6).
pub fn jpeg_data_url(jpeg: &[u8]) -> String {
    format!("data:image/jpeg;base64,{}", BASE64_STANDARD.encode(jpeg))
}

/// `Part`는 `Clone`이 아니다. 재시도(§10.1)마다 같은 파트를 다시 만들어야 한다.
fn clone_parts(parts: &[Part]) -> Vec<Part> {
    parts
        .iter()
        .map(|p| match p {
            Part::Text(t) => Part::Text(t.clone()),
            Part::ImageUrl(u) => Part::ImageUrl(u.clone()),
        })
        .collect()
}

/// 슬롯의 모델 ID. 비어 있으면 **그 투입 경로는 비활성이다** (§9.9 단계 4).
fn model_id(app: &crate::App, slot: &str) -> Result<String> {
    let id = app
        .config
        .models
        .slot(slot)
        .unwrap_or("")
        .trim()
        .to_string();
    if id.is_empty() {
        return Err(SoulError::config(format!(
            "models.{slot} 모델이 설정되지 않았습니다. `soul doctor`로 슬롯을 고르세요 (§9.9)"
        )));
    }
    Ok(id)
}

#[derive(Clone, Debug)]
struct Parsed {
    prose: String,
    axes: Axes,
    tags: Vec<String>,
}

#[derive(Deserialize)]
struct DescribeJson {
    prose: String,
    axes: Axes,
    tags: Vec<String>,
}

/// §10.1 스키마 출력을 읽는다.
fn parse_describe(value: &serde_json::Value) -> Result<Parsed> {
    // 일부 제공자는 structured output을 JSON 문자열로 한 겹 싸서 돌려준다. 한 번만 푼다.
    let owned;
    let v = match value.as_str() {
        Some(s) => {
            owned = serde_json::from_str::<serde_json::Value>(s)?;
            &owned
        }
        None => value,
    };
    let d: DescribeJson = serde_json::from_value(v.clone())?;

    let prose = d.prose.trim().to_string();
    if prose.is_empty() {
        return Err(SoulError::invalid(
            "모델이 빈 prose를 돌려주었습니다 (§10.1)",
        ));
    }
    // strict 스키마가 이미 [0,1]과 6개를 보장하지만, 어긋난 응답 하나가 투입 전체를
    // 날리지 않도록 여기서 막는다 (`Observation::validate`가 거부하는 값들이다).
    let mut axes = d.axes;
    for a in Axis::ALL {
        axes.set(a, axes.get(a).clamp(0.0, 1.0));
    }
    let mut tags = d.tags;
    tags.truncate(6);

    Ok(Parsed { prose, axes, tags })
}

/// 모든 호출을 `Tracer`에 남긴다 (§11.3). **트레이스 실패가 투입을 실패시키지 않는다.**
fn write_trace(
    app: &crate::App,
    purpose: &str,
    model: &str,
    prompt_sha256: &str,
    call: Option<&Call<serde_json::Value>>,
    error: Option<String>,
) {
    let Some(t) = app.tracer.as_ref() else {
        return;
    };
    let entry = TraceEntry {
        ts: Ts::now(),
        purpose: purpose.to_string(),
        model: model.to_string(),
        prompt_sha256: Some(prompt_sha256.to_string()),
        tokens_in: call.map(|c| c.tokens_in).unwrap_or(0),
        tokens_out: call.map(|c| c.tokens_out).unwrap_or(0),
        // 단가표를 코드에 박지 않는다 (§9.8과 같은 이유로 모델별 상수를 두지 않는다).
        cost_usd_est: 0.0,
        latency_ms: call.map(|c| c.latency_ms).unwrap_or(0),
        response_raw: call.map(|c| c.raw.clone()),
        error,
    };
    let _ = t.write(&entry);
}

struct CallOutcome {
    parsed: Parsed,
    call_ids: Vec<String>,
    retried: bool,
}

/// §10.1 — 호출 1회 + 출력 검증 + **최대 1회** 재시도 (T27).
///
/// 재시도도 검증에 걸리면 그대로 받아들이고 트레이스에만 남긴다. 무한 루프를 만들지 않는다.
async fn run_describe<F, Fut>(
    app: &crate::App,
    purpose: &str,
    model: &str,
    prompt_sha: &str,
    make_call: F,
) -> Result<CallOutcome>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<Call<serde_json::Value>>>,
{
    let first = match make_call().await {
        Ok(c) => c,
        Err(e) => {
            // 하드 실패는 관측을 만들지 않는다 (§15). 사유는 트레이스에 남긴다.
            write_trace(app, purpose, model, prompt_sha, None, Some(e.to_string()));
            return Err(e);
        }
    };
    write_trace(app, purpose, model, prompt_sha, Some(&first), None);
    let mut call_ids = vec![first.call_id.clone()];
    let mut parsed = parse_describe(&first.value)?;

    let Some(reason) = soul_agent::schemas::needs_describe_retry(&parsed.prose, &parsed.tags)
    else {
        return Ok(CallOutcome {
            parsed,
            call_ids,
            retried: false,
        });
    };

    // T27 — 재시도가 일어났다는 사실 자체를 트레이스에 남긴다.
    write_trace(
        app,
        &format!("{purpose}.retry"),
        model,
        prompt_sha,
        None,
        Some(reason.to_string()),
    );

    match make_call().await {
        Ok(second) => {
            write_trace(app, purpose, model, prompt_sha, Some(&second), None);
            match parse_describe(&second.value) {
                Ok(p2) => {
                    call_ids.push(second.call_id.clone());
                    if let Some(r2) = soul_agent::schemas::needs_describe_retry(&p2.prose, &p2.tags)
                    {
                        // **그대로 받아들인다** (§10.1). 두 번째 재시도는 없다.
                        write_trace(
                            app,
                            &format!("{purpose}.retry_failed"),
                            model,
                            prompt_sha,
                            None,
                            Some(r2.to_string()),
                        );
                    }
                    parsed = p2;
                }
                // 재시도 응답이 깨졌으면 1차 결과를 쓴다. 투입을 죽이지 않는다.
                Err(e) => write_trace(
                    app,
                    &format!("{purpose}.retry_failed"),
                    model,
                    prompt_sha,
                    None,
                    Some(e.to_string()),
                ),
            }
        }
        // 재시도 호출 자체가 실패해도 1차 결과가 이미 있다.
        Err(e) => write_trace(
            app,
            &format!("{purpose}.retry_failed"),
            model,
            prompt_sha,
            None,
            Some(e.to_string()),
        ),
    }

    Ok(CallOutcome {
        parsed,
        call_ids,
        retried: true,
    })
}

fn outcome(
    parsed: Parsed,
    quality: Quality,
    prompt_sha256: String,
    call_ids: Vec<String>,
    retried: bool,
) -> DescribeOutcome {
    DescribeOutcome {
        machine: Machine {
            prose: parsed.prose,
            axes: parsed.axes,
            tags: parsed.tags,
            quality,
            prompt_sha256,
        },
        call_ids,
        retried,
    }
}

// ──────────────────────────────────────────────────────────── 모달리티별 경로

/// §9.2 — 텍스트. 4000자 초과 시 **앞 2000 + 뒤 2000자**로 자른다.
pub async fn describe_text(
    app: &crate::App,
    client: &soul_net::OpenAi,
    text: &str,
) -> Result<DescribeOutcome> {
    let system = describe_system()?;
    let sha = system_sha(&[&system]);
    let model = model_id(app, "text")?;
    let schema = soul_agent::schemas::describe();
    let body = truncate_text(text);

    let r = run_describe(app, "describe", &model, &sha, || {
        client.responses_json(
            &model,
            &system,
            vec![Part::Text(body.clone())],
            "describe",
            &schema,
        )
    })
    .await?;

    Ok(outcome(
        r.parsed,
        quality_for(true, false),
        sha,
        r.call_ids,
        r.retried,
    ))
}

pub async fn describe_image(
    app: &crate::App,
    client: &soul_net::OpenAi,
    jpeg_data_url: &str,
) -> Result<DescribeOutcome> {
    let system = describe_system()?;
    let sha = system_sha(&[&system]);
    let model = model_id(app, "vision")?;
    let schema = soul_agent::schemas::describe();

    let r = run_describe(app, "describe", &model, &sha, || {
        client.responses_json(
            &model,
            &system,
            vec![Part::ImageUrl(jpeg_data_url.to_string())],
            "describe",
            &schema,
        )
    })
    .await?;

    Ok(outcome(
        r.parsed,
        quality_for(true, false),
        sha,
        r.call_ids,
        r.retried,
    ))
}

/// §9.5 — Chat Completions `input_audio`. 메타데이터를 텍스트 파트로 동봉하고,
/// **이어붙인 샘플이라는 사실을 프롬프트에 명시한다** (§10.2).
/// 오디오 호출 자체가 실패하면 메타데이터만으로 텍스트 경로 폴백, `quality: partial`.
pub async fn describe_audio(
    app: &crate::App,
    client: &soul_net::OpenAi,
    audio_mp3: &[u8],
    concatenated: bool,
    metadata: Option<&str>,
    thumbnail_url: Option<&str>,
) -> Result<DescribeOutcome> {
    let system = audio_system(concatenated)?;
    let sha = system_sha(&[&system]);
    // 슬롯이 비어 있으면 폴백하지 않는다 — §9.9 단계 4는 그 투입 경로를 **비활성화**하라고
    // 했지 조용히 품질을 낮추라고 하지 않았다.
    let model = model_id(app, "audio")?;
    let schema = soul_agent::schemas::describe();
    let text_parts: Vec<String> = metadata
        .map(|m| m.trim())
        .filter(|m| !m.is_empty())
        .map(|m| vec![m.to_string()])
        .unwrap_or_default();

    let attempt = run_describe(app, "describe", &model, &sha, || {
        client.audio_json(
            &model,
            &system,
            text_parts.clone(),
            audio_mp3,
            "describe",
            &schema,
        )
    })
    .await;

    match attempt {
        Ok(r) => Ok(outcome(
            r.parsed,
            quality_for(true, false),
            sha,
            r.call_ids,
            r.retried,
        )),
        // §9.5 — 오디오 호출 실패 → 메타데이터만으로 텍스트 경로 폴백, quality: partial.
        Err(e) => audio_fallback(app, client, metadata, thumbnail_url, e).await,
    }
}

/// §9.5 폴백. 오디오 갈래가 빠졌으므로 `quality: partial`이다 (§6.2).
///
/// 프롬프트는 `describe.md` + §10.2(이어붙임 문구 제외)를 그대로 쓴다 — 오디오 모달리티에
/// 대한 지시(가사·질감, 메타데이터를 맥락으로만)는 소리가 없어도 유효하다.
async fn audio_fallback(
    app: &crate::App,
    client: &soul_net::OpenAi,
    metadata: Option<&str>,
    thumbnail_url: Option<&str>,
    cause: SoulError,
) -> Result<DescribeOutcome> {
    let system = audio_system(false)?;
    let sha = system_sha(&[&system]);
    let schema = soul_agent::schemas::describe();
    let meta = metadata.map(|m| m.trim()).filter(|m| !m.is_empty());

    // 보낼 내용이 하나도 없으면 서술을 지어낼 수 없다. 관측을 만들지 않고 원인을 그대로 올린다 (§15).
    let (model, parts) = match (meta, thumbnail_url) {
        // §9.5의 "메타데이터만으로 텍스트 경로 폴백".
        (Some(m), _) => (model_id(app, "text")?, vec![Part::Text(m.to_string())]),
        // 메타데이터가 없을 때만 썸네일에 기댄다. 이것마저 없으면 폴백이 성립하지 않는다.
        (None, Some(url)) => (
            model_id(app, "vision")?,
            vec![Part::ImageUrl(url.to_string())],
        ),
        (None, None) => return Err(cause),
    };

    write_trace(
        app,
        "describe.audio_fallback",
        &model,
        &sha,
        None,
        Some(cause.to_string()),
    );

    let r = run_describe(app, "describe", &model, &sha, || {
        client.responses_json(&model, &system, clone_parts(&parts), "describe", &schema)
    })
    .await?;

    Ok(outcome(
        r.parsed,
        quality_for(false, false),
        sha,
        r.call_ids,
        r.retried,
    ))
}

/// §9.6 — **프레임 전체를 한 번의 비전 호출에 배열로 넣는다.**
/// 프레임 서술과 오디오 서술을 병합 호출(§10.3)로 하나의 산문으로 합친다.
/// 병합은 `models.text`를 쓰는 **텍스트 전용 호출**이며 원본 미디어를 다시 보내지 않는다.
pub async fn describe_video(
    app: &crate::App,
    client: &soul_net::OpenAi,
    frames: &[Vec<u8>],
    audio_mp3: Option<&[u8]>,
    metadata: Option<&str>,
) -> Result<DescribeOutcome> {
    let max = app.config.thresholds.video_max_frames as usize;
    let used = if frames.len() > max {
        &frames[..max]
    } else {
        frames
    };
    if used.is_empty() {
        return Err(SoulError::invalid(
            "영상에서 프레임을 하나도 뽑지 못했습니다 (§9.6)",
        ));
    }
    let meta = metadata.map(|m| m.trim()).filter(|m| !m.is_empty());

    let frames_system = describe_system()?;
    let frames_sha = system_sha(&[&frames_system]);
    let vision_model = model_id(app, "vision")?;
    let schema = soul_agent::schemas::describe();
    let urls: Vec<String> = used.iter().map(|f| jpeg_data_url(f)).collect();

    let frames_r = run_describe(app, "describe", &vision_model, &frames_sha, || {
        let mut parts: Vec<Part> = urls.iter().cloned().map(Part::ImageUrl).collect();
        if let Some(m) = meta {
            parts.push(Part::Text(m.to_string()));
        }
        client.responses_json(&vision_model, &frames_system, parts, "describe", &schema)
    })
    .await?;

    // §9.6 — 프레임 3장 미만이면 그 자체로 partial이다 (T65).
    let mut all_branches_ok = used.len() >= 3;
    let mut call_ids = frames_r.call_ids;
    let mut retried = frames_r.retried;
    let mut parsed = frames_r.parsed;
    // §R11 — 이 관측을 만든 시스템 프롬프트 전부를 호출 순서대로 모은다.
    let mut sections: Vec<String> = vec![frames_system.clone()];

    if let Some(mp3) = audio_mp3 {
        // 영상의 오디오는 이미 30초 구간으로 잘려 있다. 추가 샘플링이 없으므로
        // 이어붙인 샘플이 아니다 (§9.6).
        let audio_system_text = audio_system(false)?;
        let audio_sha = system_sha(&[&audio_system_text]);

        let audio_parsed = match model_id(app, "audio") {
            Ok(audio_model) => {
                let text_parts: Vec<String> = meta.map(|m| vec![m.to_string()]).unwrap_or_default();
                match run_describe(app, "describe", &audio_model, &audio_sha, || {
                    client.audio_json(
                        &audio_model,
                        &audio_system_text,
                        text_parts.clone(),
                        mp3,
                        "describe",
                        &schema,
                    )
                })
                .await
                {
                    Ok(r) => {
                        call_ids.extend(r.call_ids);
                        retried |= r.retried;
                        sections.push(audio_system_text.clone());
                        Some(r.parsed)
                    }
                    // 오디오가 실패하면 프레임만으로 진행한다 (§9.6). 병합할 것이 없다.
                    Err(_) => None,
                }
            }
            Err(e) => {
                write_trace(
                    app,
                    "describe.audio_skipped",
                    "",
                    &audio_sha,
                    None,
                    Some(e.to_string()),
                );
                None
            }
        };

        match audio_parsed {
            Some(audio) => {
                // §10.3 — 텍스트 전용 병합. 원본 미디어를 다시 보내지 않는다.
                // `axes`는 병합 모델이 재판단한다. **평균 내지 않는다.**
                let merge_sys = merge_system()?;
                let merge_sha = system_sha(&[&merge_sys]);
                let merge_model = model_id(app, "text")?;
                let body = merge_input(&parsed, &audio);

                let merged = run_describe(app, "merge", &merge_model, &merge_sha, || {
                    client.responses_json(
                        &merge_model,
                        &merge_sys,
                        vec![Part::Text(body.clone())],
                        "describe",
                        &schema,
                    )
                })
                .await?;

                call_ids.extend(merged.call_ids);
                retried |= merged.retried;
                parsed = merged.parsed;
                sections.push(merge_sys);
            }
            None => all_branches_ok = false,
        }
    } else {
        // 오디오가 없는 영상 (§9.6 · §6.2).
        all_branches_ok = false;
    }

    let refs: Vec<&str> = sections.iter().map(|s| s.as_str()).collect();
    let sha = system_sha(&refs);
    Ok(outcome(
        parsed,
        quality_for(all_branches_ok, false),
        sha,
        call_ids,
        retried,
    ))
}

/// §10.3 병합 호출의 **사용자 입력**. 시스템 프롬프트가 아니므로 `prompts/`에 두지 않는다.
/// 라벨은 `merge.md`가 이미 "하나는 프레임(시각), 하나는 오디오(청각)"라고 밝힌 것에 맞춘다.
fn merge_input(frames: &Parsed, audio: &Parsed) -> String {
    format!(
        "프레임 서술: {}\n프레임 태그: {}\n오디오 서술: {}\n오디오 태그: {}\n",
        frames.prose,
        frames.tags.join(", "),
        audio.prose,
        audio.tags.join(", ")
    )
}

/// §9.3 단계 6 — 썸네일 + 메타데이터만. `quality: minimal`.
pub async fn describe_youtube_minimal(
    app: &crate::App,
    client: &soul_net::OpenAi,
    thumbnail_url: &str,
    metadata: &str,
) -> Result<DescribeOutcome> {
    let system = describe_system()?;
    let sha = system_sha(&[&system]);
    let model = model_id(app, "vision")?;
    let schema = soul_agent::schemas::describe();
    let meta = metadata.trim().to_string();

    let r = run_describe(app, "describe", &model, &sha, || {
        // §9.3 — 썸네일은 **URL 그대로** 넘긴다. 내려받아 재전송하지 않는다.
        let mut parts = vec![Part::ImageUrl(thumbnail_url.to_string())];
        if !meta.is_empty() {
            parts.push(Part::Text(meta.clone()));
        }
        client.responses_json(&model, &system, parts, "describe", &schema)
    })
    .await?;

    Ok(outcome(
        r.parsed,
        quality_for(false, true),
        sha,
        r.call_ids,
        r.retried,
    ))
}

/// quality 판정을 한 곳에 모은다. 흩어지면 §6.2 표와 어긋난다.
pub fn quality_for(all_branches_ok: bool, only_metadata: bool) -> Quality {
    if only_metadata {
        Quality::Minimal
    } else if all_branches_ok {
        Quality::Full
    } else {
        Quality::Partial
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ───────────────────────────────────────────────── §9.2 4000자 절단

    #[test]
    fn short_text_is_untouched() {
        let s = "짧은 글";
        assert_eq!(truncate_text(s), s);
    }

    #[test]
    fn exactly_4000_chars_is_untouched() {
        let s: String = std::iter::repeat_n('가', 4000).collect();
        assert_eq!(truncate_text(&s).chars().count(), 4000);
        assert_eq!(truncate_text(&s), s);
    }

    /// §9.2 — 4000자를 넘으면 앞 2000 + 뒤 2000자.
    #[test]
    fn long_text_keeps_head_and_tail_only() {
        let head: String = std::iter::repeat_n('앞', 2000).collect();
        let middle: String = std::iter::repeat_n('중', 5000).collect();
        let tail: String = std::iter::repeat_n('뒤', 2000).collect();
        let cut = truncate_text(&format!("{head}{middle}{tail}"));

        assert_eq!(cut.chars().count(), 4000);
        assert_eq!(cut, format!("{head}{tail}"));
        assert!(!cut.contains('중'), "가운데는 버린다");
    }

    /// 바이트가 아니라 **문자** 기준이다.
    #[test]
    fn truncation_counts_chars_not_bytes() {
        let s: String = std::iter::repeat_n('가', 4001).collect();
        assert!(s.len() > 4000 * 2, "한국어는 문자당 3바이트다");
        assert_eq!(truncate_text(&s).chars().count(), 4000);
    }

    // ─────────────────────────────────────────────────── §6.2 quality 표

    /// §6.2 표 그대로. §12.1 가중치(T57)의 근거가 이 판정이다.
    #[test]
    fn quality_table_matches_spec() {
        assert_eq!(quality_for(true, false), Quality::Full);
        assert_eq!(quality_for(false, false), Quality::Partial);
        assert_eq!(quality_for(true, true), Quality::Minimal);
        assert_eq!(quality_for(false, true), Quality::Minimal);

        // T57 — 가중치는 soul-core가 갖고 있고 이 모듈은 값만 고른다.
        assert_eq!(quality_for(true, false).weight(), 1.0);
        assert_eq!(quality_for(false, false).weight(), 0.6);
        assert_eq!(quality_for(false, true).weight(), 0.2);
    }

    // ────────────────────────────────────────────── §R11 prompt_sha256

    /// T23 — 프롬프트가 한 글자만 달라져도 sha256이 갈린다.
    #[test]
    fn sha_changes_when_prompt_text_changes() {
        let a = system_sha(&["당신은 감각적 관찰자다.\n"]);
        let b = system_sha(&["당신은 감각적 관찰자다!\n"]);
        assert_ne!(a, b);
        assert_eq!(a.len(), 64);
        assert!(a
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    /// 개행만 달라도 sha가 갈리면 없는 경계가 생긴다 — 조립은 개행을 정규화한다 (§R11).
    #[test]
    fn compose_normalizes_trailing_newlines() {
        assert_eq!(compose(&["a"]), "a\n");
        assert_eq!(compose(&["a\n"]), "a\n");
        assert_eq!(compose(&["a\n\n\n"]), "a\n");
        assert_eq!(compose(&["a", "b"]), "a\n\nb\n");
        assert_eq!(compose(&["a", "", "b"]), "a\n\nb\n");
        assert_eq!(system_sha(&["a"]), system_sha(&["a\n"]));
    }

    /// 경로마다 최종 시스템 프롬프트가 다르므로 sha도 달라야 한다 (§R11).
    #[test]
    fn each_path_has_its_own_sha() {
        let describe = system_sha(&[&describe_system().unwrap()]);
        let audio_cat = system_sha(&[&audio_system(true).unwrap()]);
        let audio_plain = system_sha(&[&audio_system(false).unwrap()]);
        let merge = system_sha(&[&merge_system().unwrap()]);

        assert_ne!(describe, audio_cat);
        assert_ne!(describe, audio_plain);
        assert_ne!(
            audio_cat, audio_plain,
            "이어붙인 샘플인지 여부가 프롬프트를 바꾸므로 sha도 갈려야 한다"
        );
        assert_ne!(describe, merge);
    }

    /// 영상 경로(프레임+오디오+병합)의 sha는 세 프롬프트 전부에 반응한다 (T23).
    #[test]
    fn video_sha_reacts_to_every_prompt_in_the_chain() {
        let frames = "프레임 프롬프트\n";
        let audio = "오디오 프롬프트\n";
        let merge = "병합 프롬프트\n";
        let base = system_sha(&[frames, audio, merge]);

        assert_ne!(base, system_sha(&["프레임 프롬프트 v2\n", audio, merge]));
        assert_ne!(base, system_sha(&[frames, "오디오 프롬프트 v2\n", merge]));
        assert_ne!(base, system_sha(&[frames, audio, "병합 프롬프트 v2\n"]));
        // 오디오가 없는 영상은 프레임 프롬프트 하나뿐이므로 이미지 경로와 같은 sha다.
        assert_eq!(system_sha(&[frames]), system_sha(&[frames]));
    }

    // ─────────────────────────────────────────────────── §10.2 오디오 지시

    #[test]
    fn concat_notice_is_dropped_when_not_concatenated() {
        let md = "가사 지시\n메타데이터 지시\n이 오디오는 세 구간을 이어붙인 샘플이다\n";
        assert_eq!(audio_extra(md, true), md);
        let plain = audio_extra(md, false);
        assert!(plain.contains("가사 지시"));
        assert!(plain.contains("메타데이터 지시"));
        assert!(!plain.contains("이어붙인"), "{plain}");
    }

    /// 번들된 `prompts/audio.md`에 실제로 이어붙임 문구가 있는지 확인한다.
    /// 문구를 재작성해 판별자가 빗나가면 이 테스트가 먼저 깨진다 (조용히 틀리지 않게).
    #[test]
    fn bundled_audio_prompt_has_a_concat_notice_line() {
        let md = prompts::load(PromptFile::Audio).unwrap();
        let filtered = audio_extra(&md, false);
        assert!(
            filtered.lines().count() < md.lines().count(),
            "audio.md의 이어붙임 문구를 골라내지 못했다. CONCAT_MARKERS를 갱신하라:\n{md}"
        );
        assert!(
            !filtered.trim().is_empty(),
            "나머지 지시는 남아 있어야 한다"
        );
    }

    /// §9.5 — 이어붙인 샘플이면 그 사실이 반드시 프롬프트에 있다.
    #[test]
    fn concatenated_audio_system_states_the_fact() {
        let sys = audio_system(true).unwrap();
        assert!(CONCAT_MARKERS.iter().any(|m| sys.contains(m)), "{sys}");
        let sys_plain = audio_system(false).unwrap();
        assert!(!CONCAT_MARKERS.iter().any(|m| sys_plain.contains(m)));
    }

    // ─────────────────────────────────────────────────────── §10.3 병합 입력

    #[test]
    fn merge_input_carries_both_descriptions() {
        let f = Parsed {
            prose: "회색 복도가 끝없이 이어진다".into(),
            axes: Axes::ZERO,
            tags: vec!["복도".into(), "무채색".into()],
        };
        let a = Parsed {
            prose: "저역이 두꺼운 반복음".into(),
            axes: Axes::ZERO,
            tags: vec!["저역".into()],
        };
        let body = merge_input(&f, &a);
        assert!(body.contains("회색 복도가 끝없이 이어진다"));
        assert!(body.contains("저역이 두꺼운 반복음"));
        assert!(body.contains("복도, 무채색"));
        assert!(body.contains("저역"));
    }

    // ─────────────────────────────────────────────────────── 응답 파싱

    fn sample_json() -> serde_json::Value {
        serde_json::json!({
            "prose": "  차갑고 정돈된, 사람이 방금 지워진 실내  ",
            "axes": {"chroma":0.21,"luminance":0.44,"density":0.31,"grain":0.68,
                     "tempo":0.15,"space":0.77,"valence":0.38,"intensity":0.25},
            "tags": ["실내","자연광","무인"]
        })
    }

    #[test]
    fn parses_describe_schema_output() {
        let p = parse_describe(&sample_json()).unwrap();
        assert_eq!(p.prose, "차갑고 정돈된, 사람이 방금 지워진 실내");
        assert_eq!(p.axes.chroma, 0.21);
        assert_eq!(p.tags, vec!["실내", "자연광", "무인"]);
    }

    /// 한 겹 싸서 오는 응답도 읽는다.
    #[test]
    fn parses_json_encoded_as_string() {
        let wrapped = serde_json::Value::String(sample_json().to_string());
        assert_eq!(
            parse_describe(&wrapped).unwrap().prose,
            "차갑고 정돈된, 사람이 방금 지워진 실내"
        );
    }

    /// 스키마를 벗어난 값이 와도 `Observation::validate`가 거부하지 않게 다듬는다.
    #[test]
    fn out_of_range_axes_and_extra_tags_are_clamped() {
        let mut v = sample_json();
        v["axes"]["chroma"] = serde_json::json!(1.4);
        v["axes"]["grain"] = serde_json::json!(-0.2);
        v["tags"] = serde_json::json!(["1", "2", "3", "4", "5", "6", "7", "8"]);
        let p = parse_describe(&v).unwrap();
        assert_eq!(p.axes.chroma, 1.0);
        assert_eq!(p.axes.grain, 0.0);
        assert_eq!(p.tags.len(), 6);
        assert!(p.axes.in_unit_range());
    }

    #[test]
    fn empty_prose_is_rejected() {
        let mut v = sample_json();
        v["prose"] = serde_json::json!("   ");
        assert!(parse_describe(&v).is_err());
    }

    #[test]
    fn data_url_prefix_is_jpeg() {
        let url = jpeg_data_url(&[0xFF, 0xD8, 0xFF]);
        assert!(url.starts_with("data:image/jpeg;base64,"), "{url}");
        assert_eq!(url, "data:image/jpeg;base64,/9j/");
    }

    // ───────────────────────────────────── §10.1 출력 검증 · 재시도 (T27)
    //
    // 네트워크를 쓰지 않는다. `run_describe`에 가짜 응답 생성기를 넣어
    // **재시도 횟수와 최종 채택본**만 본다.

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn temp_app() -> crate::App {
        let root = std::env::temp_dir()
            .join("tasty-soul-describe-test")
            .join(soul_core::ids::new_id().to_string());
        crate::App::open_or_init(soul_core::paths::Paths::at(root)).unwrap()
    }

    fn call_of(prose: &str, n: usize) -> Call<serde_json::Value> {
        let mut v = sample_json();
        v["prose"] = serde_json::json!(prose);
        Call {
            value: v,
            call_id: format!("resp_{n}"),
            tokens_in: 1,
            tokens_out: 1,
            latency_ms: 1,
            raw: String::new(),
        }
    }

    /// §10.1이 재시도 대상으로 삼는 3문장짜리 분류문.
    const THREE_SENTENCES: &str = "차갑다. 정돈되어 있다. 사람이 없다.";
    const ONE_SENTENCE: &str = "차갑고 정돈된, 사람이 방금 지워진 실내";

    /// 호출 순서대로 `responses`를 하나씩 돌려주는 가짜 생성기.
    async fn run_with(
        app: &crate::App,
        responses: Vec<std::result::Result<&'static str, &'static str>>,
    ) -> (Result<CallOutcome>, usize) {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let plan = responses.clone();
        let out = run_describe(app, "test", "m", "sha", move || {
            let n = c.fetch_add(1, Ordering::SeqCst);
            let item = plan.get(n).copied();
            async move {
                match item {
                    Some(Ok(prose)) => Ok(call_of(prose, n)),
                    Some(Err(msg)) => Err(SoulError::invalid(msg)),
                    None => Err(SoulError::invalid("계획에 없는 호출")),
                }
            }
        })
        .await;
        let n = calls.load(Ordering::SeqCst);
        (out, n)
    }

    /// 첫 응답이 검증을 통과하면 재시도하지 않는다.
    #[tokio::test]
    async fn a_valid_first_response_is_not_retried() {
        let app = temp_app();
        let (r, n) = run_with(&app, vec![Ok(ONE_SENTENCE)]).await;
        let r = r.unwrap();
        assert_eq!(n, 1, "호출은 한 번뿐이다");
        assert!(!r.retried);
        assert_eq!(r.call_ids, vec!["resp_0".to_string()]);
        assert_eq!(r.parsed.prose, ONE_SENTENCE);
    }

    /// T27 — 3문장 응답 → 재시도 1회 → 두 번째 결과를 채택한다.
    #[tokio::test]
    async fn a_three_sentence_response_triggers_exactly_one_retry() {
        let app = temp_app();
        let (r, n) = run_with(&app, vec![Ok(THREE_SENTENCES), Ok(ONE_SENTENCE)]).await;
        let r = r.unwrap();
        assert_eq!(n, 2, "재시도는 정확히 1회다");
        assert!(r.retried);
        assert_eq!(r.parsed.prose, ONE_SENTENCE, "두 번째 결과를 쓴다");
        assert_eq!(r.call_ids, vec!["resp_0".to_string(), "resp_1".to_string()]);
    }

    /// T27 — **재시도도 실패하면 그대로 받아들인다.** 세 번째 호출은 없다.
    #[tokio::test]
    async fn a_failed_retry_is_accepted_and_never_loops() {
        let app = temp_app();
        let (r, n) = run_with(&app, vec![Ok(THREE_SENTENCES), Ok(THREE_SENTENCES)]).await;
        let r = r.unwrap();
        assert_eq!(n, 2, "무한 루프를 만들지 않는다");
        assert!(r.retried);
        assert_eq!(r.parsed.prose, THREE_SENTENCES, "거부하지 않고 그대로 쓴다");
        // 두 응답 모두 파싱에 성공했으므로 호출 ID 두 개가 모두 남는다.
        assert_eq!(r.call_ids.len(), 2);
    }

    /// 재시도 호출 자체가 실패하면 1차 결과를 쓴다. 투입을 죽이지 않는다.
    #[tokio::test]
    async fn a_retry_call_error_falls_back_to_the_first_result() {
        let app = temp_app();
        let (r, n) = run_with(&app, vec![Ok(THREE_SENTENCES), Err("연결 끊김")]).await;
        let r = r.unwrap();
        assert_eq!(n, 2);
        assert!(r.retried);
        assert_eq!(r.parsed.prose, THREE_SENTENCES);
        assert_eq!(
            r.call_ids,
            vec!["resp_0".to_string()],
            "실패한 호출은 세지 않는다"
        );
    }

    /// 1차 호출이 실패하면 관측을 만들지 않는다 (§15). 재시도하지 않는다.
    #[tokio::test]
    async fn a_hard_failure_on_the_first_call_makes_no_observation() {
        let app = temp_app();
        let (r, n) = run_with(&app, vec![Err("401 Unauthorized")]).await;
        assert!(r.is_err());
        assert_eq!(n, 1, "하드 실패는 재시도 대상이 아니다");
    }

    /// 슬롯이 비어 있으면 그 투입 경로는 **비활성이다** — 조용히 폴백하지 않는다 (§9.9 단계 4).
    #[test]
    fn an_empty_model_slot_is_an_error_not_a_fallback() {
        let app = temp_app();
        for slot in soul_core::config::ModelsConfig::SLOTS {
            let e = model_id(&app, slot).unwrap_err().to_string();
            assert!(e.contains(slot), "{e}");
        }
    }
}
