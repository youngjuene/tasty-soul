//! 투입 (§9.1).
//!
//! 순서를 지킨다: 서술 → 임베딩 → `min_dist`·`surprisal` → **관측 기록** → 큐 등록.
//! **`ingest` 관측은 카드 표시 전에 이미 기록되어 있다** (§13 화면 2).
//! 카드에 답하지 않아도 투입은 유효하다.
//!
//! **임베딩 실패 시 관측을 만들지 않는다** (§15).

use crate::describe;
use sha2::{Digest, Sha256};
use soul_core::db::embed_cache::Space;
use soul_core::db::Db;
use soul_core::derived::cluster::{self, Clustering};
use soul_core::derived::surprisal::{self, Surprisal};
use soul_core::error::{Result, SoulError};
use soul_core::git;
use soul_core::ids::ObsId;
use soul_core::lock::WriteLock;
use soul_core::obs::{Ingest, Kind, Machine, ModelRef, ObsSet, Observation, Source};
use soul_core::time::Ts;
use soul_media::probe::{self, DetectedKind};
use soul_net::youtube::{self, YoutubeMeta};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

pub enum IngestInput {
    /// 클립보드 텍스트 (§9.2).
    Text(String),
    /// 로컬 파일 (이미지·영상).
    File(std::path::PathBuf),
    /// YouTube 링크 (§9.3). `forced_kind`는 `soul recast`에서만 쓴다.
    Youtube {
        url: String,
        forced_kind: Option<Kind>,
    },
}

#[derive(Debug)]
pub struct IngestOutcome {
    pub id: ObsId,
    pub kind: Kind,
    pub prose: String,
    pub thumbnail: Option<std::path::PathBuf>,
    /// §9.3 — YouTube 항목은 카드에서 한 번의 탭으로 kind를 뒤집을 수 있다.
    pub kind_is_guess: bool,
    /// 비평 큐에 등록되었는가. `context_enabled = false`면 false (T54).
    pub queued_for_critique: bool,
}

/// 진행 단계 알림 (§13 화면 1 — "프레임 추출 → 오디오 변환 → 서술 → 병합").
pub type ProgressFn<'a> = &'a (dyn Fn(&str) + Send + Sync);

/// §13 화면 1에 그대로 뜨는 문자열. 한 곳에 모은다.
pub mod stage {
    pub const DETECT: &str = "판별";
    pub const YOUTUBE: &str = "YouTube 해석";
    pub const DOWNLOAD: &str = "다운로드";
    pub const IMAGE: &str = "이미지 변환";
    pub const FRAMES: &str = "프레임 추출";
    pub const AUDIO: &str = "오디오 변환";
    pub const THUMBNAIL: &str = "썸네일";
    pub const DESCRIBE: &str = "서술";
    pub const EMBED: &str = "임베딩";
    pub const RECORD: &str = "기록";
}

pub async fn ingest(
    app: &crate::App,
    input: IngestInput,
    progress: Option<ProgressFn<'_>>,
) -> Result<IngestOutcome> {
    // 1. kind 판별 (§9.1). **확장자를 신뢰하지 않는다** — 판별은 전부 `soul-media`에 있다.
    note(progress, stage::DETECT);
    let plan = match input {
        IngestInput::Text(text) => match probe::detect_clipboard(&text) {
            DetectedKind::Accepted { .. } => Plan::Text(text),
            DetectedKind::Rejected { reason } => return Err(SoulError::invalid(reason)),
            // 2. YouTube면 §9.3 해석기로 넘긴다. kind는 거기서 정해진다.
            DetectedKind::YouTube { video_id, .. } => {
                note(progress, stage::YOUTUBE);
                resolve_youtube(app, &video_id, None).await?
            }
        },
        IngestInput::File(path) => file_plan(&path)?,
        IngestInput::Youtube { url, forced_kind } => {
            let video_id = probe::youtube_video_id(&url)
                .ok_or_else(|| SoulError::invalid(format!("YouTube 링크가 아닙니다: {url}")))?;
            note(progress, stage::YOUTUBE);
            resolve_youtube(app, &video_id, forced_kind).await?
        }
    };
    run(app, plan, None, progress).await
}

/// `soul reanalyze <id>` (§R9) — 기존 `ingest`와 동일한 `source`로 새 관측을 만들고
/// `supersedes`를 넣는다. **기존 관측 파일은 불변이다** (T16).
///
/// `origin` 경로에 파일이 아직 있고 sha256이 일치할 때만 가능하다.
/// 없으면 그 사실을 **명확히 알리고 중단한다** (§20.4).
pub async fn reanalyze(app: &crate::App, id: &ObsId) -> Result<IngestOutcome> {
    let previous = app.store.read(id)?;
    let previous = previous.as_ingest().ok_or_else(|| {
        SoulError::invalid(format!(
            "{id} 은(는) ingest 관측이 아닙니다. 재분석할 수 없습니다"
        ))
    })?;
    let plan = plan_from_source(app, &previous.source).await?;
    run(app, plan, Some(id.clone()), None).await
}

// ─────────────────────────────────────────────────────── 해석이 끝난 투입 원재료

/// kind 판별과 §9.3 해석까지 끝난 상태. `ingest`·`reanalyze`·`recast`가 **같은 경로**를 탄다.
pub(crate) enum Plan {
    /// 붙여넣은 텍스트 (§9.2).
    Text(String),
    /// 로컬 파일. `mime`은 **변환 결과가 아니라 원본 파일의 mime**이다 (§9.4).
    File {
        path: PathBuf,
        kind: Kind,
        mime: String,
    },
    /// §9.3 해석이 끝난 YouTube 항목.
    Youtube {
        meta: Box<YoutubeMeta>,
        kind: Kind,
        /// 카테고리로 **추정**한 값인가. `recast`·`reanalyze`가 kind를 지정하면 false다.
        kind_is_guess: bool,
    },
}

/// §9.3 단계 1~4 + kind 추정. `forced`가 있으면 추정하지 않는다 (`recast`·`reanalyze`).
pub(crate) async fn resolve_youtube(
    app: &crate::App,
    video_id: &str,
    forced: Option<Kind>,
) -> Result<Plan> {
    let key = trimmed(&app.config.youtube.api_key);
    let meta = youtube::resolve(video_id, key, app.config.api.timeout_secs).await?;
    // 카테고리 10(Music) → audio, 그 외/미상 → video (§9.3). 판정은 `soul-net`에 있다.
    let kind = forced.unwrap_or_else(|| meta.guess_kind());
    Ok(Plan::Youtube {
        meta: Box::new(meta),
        kind,
        kind_is_guess: forced.is_none(),
    })
}

fn file_plan(path: &Path) -> Result<Plan> {
    let path = std::path::absolute(path)?;
    if !path.is_file() {
        return Err(SoulError::MissingPath(path));
    }
    match probe::detect_kind(&path) {
        DetectedKind::Accepted { kind, mime } => Ok(Plan::File { path, kind, mime }),
        DetectedKind::Rejected { reason } => Err(SoulError::invalid(reason)),
        DetectedKind::YouTube { .. } => Err(SoulError::invalid(
            "파일 경로에서 YouTube 링크가 나올 수 없습니다",
        )),
    }
}

/// §20.4 — 재분석은 `origin`이 가리키는 원본이 **아직 있고 sha256이 일치할 때만** 가능하다.
async fn plan_from_source(app: &crate::App, source: &Source) -> Result<Plan> {
    if let Some(video_id) = probe::youtube_video_id(&source.origin) {
        // "동일한 `source`로"(§R9) — kind를 다시 추정하지 않는다.
        return resolve_youtube(app, &video_id, Some(source.kind)).await;
    }
    if source.origin.starts_with("clipboard:") {
        return Err(SoulError::invalid(
            "붙여넣은 텍스트는 원본을 보관하지 않으므로 재분석할 수 없습니다 \
             (§20.4 — 이 앱은 사본 저장소가 아닙니다)",
        ));
    }
    let Some(path) = origin_to_path(&source.origin) else {
        return Err(SoulError::invalid(format!(
            "알 수 없는 origin 형식입니다: {}",
            source.origin
        )));
    };
    if !path.is_file() {
        return Err(SoulError::invalid(format!(
            "원본 파일이 없어 재분석할 수 없습니다: {}. \
             원본 보관은 사용자 책임이며 이 앱은 사본 저장소가 아닙니다 (§20.4)",
            path.display()
        )));
    }
    let sha = sha256_file(&path)?;
    if sha != source.sha256 {
        return Err(SoulError::invalid(format!(
            "원본 파일이 바뀌어 재분석할 수 없습니다: {} (sha256 {} ≠ 기록된 {})",
            path.display(),
            short(&sha),
            short(&source.sha256)
        )));
    }
    // kind·mime은 기록된 값을 그대로 쓴다. 바이트가 같으므로 다시 판별할 이유가 없다.
    match source.kind {
        Kind::Image | Kind::Video => Ok(Plan::File {
            path,
            kind: source.kind,
            mime: source.mime.clone(),
        }),
        other => Err(SoulError::invalid(format!(
            "파일 원본의 kind가 {other} 입니다. 재분석 경로가 없습니다"
        ))),
    }
}

// ────────────────────────────────────────────────────────────── 본 흐름 (§9.1)

/// 3~9단계를 한 줄기로 태운다. `supersedes`가 있으면 `reanalyze`·`recast` 경로다 (§R9).
pub(crate) async fn run(
    app: &crate::App,
    plan: Plan,
    supersedes: Option<ObsId>,
    progress: Option<ProgressFn<'_>>,
) -> Result<IngestOutcome> {
    let cfg = &app.config;
    let client = app.openai()?;

    // 3~5단계: sha256 → 썸네일 → 모달리티별 서술.
    let (source, kind_is_guess, thumbnail, described, model_id) = match plan {
        Plan::Text(text) => {
            let source = text_source(&text);
            // §20.4 — text는 썸네일이 없다. 아카이브는 서술문 앞 40자를 렌더한다 (T70c).
            note(progress, stage::DESCRIBE);
            let described = describe::describe_text(app, &client, &text).await?;
            (source, false, None, described, cfg.models.text.clone())
        }

        Plan::File { path, kind, mime } => {
            let source = file_source(&path, kind, mime)?;
            match kind {
                Kind::Image => {
                    note(progress, stage::IMAGE);
                    let tools = soul_media::ffmpeg::locate(&app.paths.bin());
                    let prepared = soul_media::image_in::prepare(
                        &path,
                        cfg.thresholds.image_max_edge_px,
                        tools.as_ref(),
                    )?;
                    // §20.4 — image의 썸네일 원본은 원본 이미지다.
                    note(progress, stage::THUMBNAIL);
                    let thumb = write_thumb(app, &source.sha256, &prepared.jpeg);
                    note(progress, stage::DESCRIBE);
                    let url = soul_media::image_in::to_data_url(&prepared.jpeg);
                    let described = describe::describe_image(app, &client, &url).await?;
                    (source, false, thumb, described, cfg.models.vision.clone())
                }
                Kind::Video => {
                    let tools = require_tools(app)?;
                    note(progress, stage::FRAMES);
                    let prepared = soul_media::video::prepare(
                        &tools,
                        &path,
                        cfg.thresholds.video_max_seconds,
                        cfg.thresholds.video_fps,
                        cfg.thresholds.video_max_frames,
                        cfg.thresholds.image_max_edge_px,
                    )?;
                    // §20.4 — 로컬 video의 썸네일 원본은 **추출한 첫 프레임**이다.
                    note(progress, stage::THUMBNAIL);
                    let thumb = prepared
                        .first_frame
                        .as_deref()
                        .and_then(|f| write_thumb(app, &source.sha256, f));
                    note(progress, stage::DESCRIBE);
                    let described = describe::describe_video(
                        app,
                        &client,
                        &prepared.frames,
                        prepared.audio_mp3.as_deref(),
                        None,
                    )
                    .await?;
                    (source, false, thumb, described, cfg.models.vision.clone())
                }
                other => {
                    return Err(SoulError::invalid(format!(
                        "파일 투입은 image·video만 받습니다 (판별 결과: {other})"
                    )))
                }
            }
        }

        Plan::Youtube {
            meta,
            kind,
            kind_is_guess,
        } => {
            let source = youtube_source(kind, &meta);
            // §20.4 · T11e — 아카이브용 로컬 사본을 **이때만 한 번** 내려받는다.
            // 서술용으로는 URL을 그대로 넘긴다 (§9.3 · §D6).
            note(progress, stage::THUMBNAIL);
            let thumb = youtube_thumbnail(app, &source.sha256, &meta).await;

            // 단계 5 — yt-dlp. 실패는 에러가 아니라 단계 6으로 가는 정상 경로다 (§15, T11).
            let clip = if cfg.youtube.download_enabled {
                note(progress, stage::DOWNLOAD);
                let seconds = match kind {
                    Kind::Audio => cfg.thresholds.audio_cap_seconds,
                    _ => cfg.thresholds.video_max_seconds,
                };
                match youtube::download_clip(
                    &meta.video_id,
                    kind,
                    &app.paths.media_cache(),
                    seconds,
                )
                .await
                {
                    Ok(clip) => clip,
                    // **사용자에게 오류를 띄우지 않는다.** 트레이스에만 남긴다 (§9.3).
                    Err(e) => {
                        trace_error(app, "youtube_download", e.to_string());
                        None
                    }
                }
            } else {
                None
            };

            let metadata = youtube_metadata_text(&meta);
            match clip {
                // 단계 6 — 썸네일 + 메타데이터만. `quality: minimal`.
                None => {
                    note(progress, stage::DESCRIBE);
                    let described = describe::describe_youtube_minimal(
                        app,
                        &client,
                        &meta.thumbnail_url,
                        &metadata,
                    )
                    .await?;
                    (
                        source,
                        kind_is_guess,
                        thumb,
                        described,
                        cfg.models.vision.clone(),
                    )
                }
                Some(clip) => match kind {
                    Kind::Audio => {
                        let tools = require_tools(app)?;
                        note(progress, stage::AUDIO);
                        // 캐시된 클립이 영상 컨테이너여도 ffmpeg이 오디오만 뽑는다 (T11j).
                        let audio = soul_media::audio::prepare(
                            &tools,
                            &clip.path,
                            cfg.thresholds.audio_cap_seconds,
                        )?;
                        note(progress, stage::DESCRIBE);
                        let described = describe::describe_audio(
                            app,
                            &client,
                            &audio.mp3,
                            audio.concatenated,
                            Some(&metadata),
                            Some(&meta.thumbnail_url),
                        )
                        .await?;
                        (
                            source,
                            kind_is_guess,
                            thumb,
                            described,
                            cfg.models.audio.clone(),
                        )
                    }
                    _ => {
                        let tools = require_tools(app)?;
                        note(progress, stage::FRAMES);
                        let prepared = soul_media::video::prepare(
                            &tools,
                            &clip.path,
                            cfg.thresholds.video_max_seconds,
                            cfg.thresholds.video_fps,
                            cfg.thresholds.video_max_frames,
                            cfg.thresholds.image_max_edge_px,
                        )?;
                        note(progress, stage::DESCRIBE);
                        let described = if prepared.frames.is_empty() {
                            // 캐시된 클립이 오디오 전용이면 프레임이 없다. **yt-dlp를 다시
                            // 부르지 않고**(T11j) §9.3 단계 6으로 내려간다.
                            describe::describe_youtube_minimal(
                                app,
                                &client,
                                &meta.thumbnail_url,
                                &metadata,
                            )
                            .await?
                        } else {
                            describe::describe_video(
                                app,
                                &client,
                                &prepared.frames,
                                prepared.audio_mp3.as_deref(),
                                Some(&metadata),
                            )
                            .await?
                        };
                        (
                            source,
                            kind_is_guess,
                            thumb,
                            described,
                            cfg.models.vision.clone(),
                        )
                    }
                },
            }
        }
    };

    // 6. embed(machine.prose). **실패하면 관측을 만들지 않는다** (§15).
    //    임베딩 대상은 `machine.prose` 문자열 단독이다 — tags·axes를 이어붙이지 않는다 (§9.1).
    note(progress, stage::EMBED);
    let vector = embedder(app, &client).get(&described.machine.prose).await?;

    // 7. min_dist·surprisal (§12.4). 캐시된 중심을 쓴다 (§12.3, T46).
    let set = app.store.load_set()?;
    let frozen = compute_frozen(app, &vector, &set)?;

    // 8~9. 관측 기록 + 커밋 1개 + 큐 등록. **재렌더하지 않는다** (§R8, T21b).
    note(progress, stage::RECORD);
    let kind = source.kind;
    let prose = described.machine.prose.clone();
    let model = ModelRef {
        provider: soul_core::rebuild::EMBED_PROVIDER.to_string(),
        id: model_id,
        // `machine.prompt_sha256`이 이미 §R11의 값을 들고 있다 (§6.2의 ingest 예시와 동일).
        prompt_sha256: None,
        calls: described.call_ids,
    };
    let (id, queued) = record_ingest(
        app,
        source,
        described.machine,
        model,
        frozen,
        &vector,
        supersedes,
    )?;

    Ok(IngestOutcome {
        id,
        kind,
        prose,
        thumbnail,
        kind_is_guess,
        queued_for_critique: queued,
    })
}

/// 8~9단계 — 관측 기록·커밋·파생 캐시 사영·비평 큐 (§R8 · §R4 · §9.10).
///
/// **네트워크를 쓰지 않는다.** 오프라인 테스트가 이 함수를 직접 부른다.
/// **`SOUL.md`를 재렌더하지 않는다** (§R8의 재렌더 계기 넷에 `ingest`는 없다, T21b).
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_ingest(
    app: &crate::App,
    source: Source,
    machine: Machine,
    model: ModelRef,
    frozen: Surprisal,
    embedding: &[f32],
    supersedes: Option<ObsId>,
) -> Result<(ObsId, bool)> {
    let (id, ts, schema) = soul_core::obs::new_header();
    let obs = Observation::Ingest(Ingest {
        id: id.clone(),
        ts,
        schema,
        source,
        machine,
        min_dist: frozen.min_dist,
        surprisal: frozen.surprisal,
        model,
        supersedes: supersedes.clone(),
    });

    // §R7 — 쓰기는 락 안에서. 획득 실패는 즉시 에러다(대기하지 않는다).
    // 락은 쓰기 구간에서만 잡는다. API 호출 내내 쥐고 있으면 비평 워커가 굶는다.
    {
        let _lock = WriteLock::acquire(&app.paths.soul())?;
        let path = app.store.append(&obs)?;
        // §R8 — 쓰기 1회 = 커밋 1개. 메시지는 `<type> <ULID>`.
        git::commit_paths(&app.paths.soul(), &[&path], &format!("ingest {id}"))?;
    }

    // 파생 캐시는 관측의 **사영**이다 (§R4). 원본은 방금 쓴 JSON이다.
    app.db.obs_vec_put(Space::Object, id.as_str(), embedding)?;
    app.db
        .ingest_frozen_put(id.as_str(), frozen.min_dist, frozen.surprisal, ts)?;
    if let Some(old) = &supersedes {
        // §R9 · T17 — supersede된 ingest는 §12의 모든 계산에서 빠진다.
        app.db.ingest_frozen_remove(old.as_str())?;
    }
    // §20.7 — 관측이 들어간 **그 달 하나만** 무효화한다 (T40).
    app.db.month_state_invalidate(ts.month())?;

    // 9. 비평 큐 (§9.10). `context_enabled = false`면 등록하지 않는다 (T54).
    let queued = enqueue_critique(app, &id)?;
    Ok((id, queued))
}

/// §9.10 — `ingest` 기록 직후의 큐 등록 (T51b). `context_enabled = false`면 아무것도 하지 않는다 (T54).
pub(crate) fn enqueue_critique(app: &crate::App, id: &ObsId) -> Result<bool> {
    if !app.config.thresholds.context_enabled {
        return Ok(false);
    }
    app.db.queue_push(id.as_str())?;
    Ok(true)
}

// ─────────────────────────────────────────────────────── §12.4 동결값 계산

/// §12.4 — `P`는 **이 투입 직전까지의** ingest 집합이다.
///
/// 이 시점에는 아직 `supersedes` 간선이 없으므로 재분석 대상도 `P`에 남아 있다.
/// 관측을 쓴 뒤 `record_ingest`가 `ingest_frozen`에서 그것을 지워, 그 다음 계산부터
/// 빠진다 (§R9, T17). 어차피 두 값은 동결되므로(§R4) 이 경계에서만 의미가 있다.
pub(crate) fn compute_frozen(app: &crate::App, new_vec: &[f32], set: &ObsSet) -> Result<Surprisal> {
    // §R9 — 이미 supersede된 것은 언제나 제외한다. `active_ingests`가 유일한 관문이다.
    let active = set.active_ingests();
    let ids: Vec<&str> = active.iter().map(|i| i.id.as_str()).collect();
    let (clustering, _) = clustering_for(&app.db, &ids)?;
    // `D` = null이 아닌 동결 min_dist들. `|D| = 0`이어도 나눗셈을 하지 않는다 (T18).
    let prior = app.db.min_dists_get()?;
    Ok(surprisal::compute(new_vec, clustering.as_ref(), &prior))
}

/// §12.3 — 캐시된 중심을 쓰고 `should_recluster`가 참일 때만 다시 만든다 (T46).
///
/// 반환값의 두 번째는 "이번에 재계산했는가"다. 테스트가 이 값을 센다.
pub(crate) fn clustering_for(db: &Db, active_ids: &[&str]) -> Result<(Option<Clustering>, bool)> {
    let n = active_ids.len();
    if let Some((cached_n, cached)) = db.cluster_get()? {
        if !surprisal::should_recluster(cached_n, n) {
            return Ok((Some(cached), false));
        }
    }
    // §R5 — 입력은 **ULID 오름차순**이어야 결정론이 성립한다 (T12).
    // `active_ingests()`가 이미 그 순서다.
    //
    // `obs_vec`에는 감각 `reading`의 정정문 벡터도 들어 있다(`Space::for_reading`).
    // 활성 ingest ID로 걸러야 군집이 "무엇을 좋아하는가"에서 흐르지 않는다 (§12.7, T49).
    let stored: HashMap<String, Vec<f32>> = db.obs_vec_all(Space::Object)?.into_iter().collect();
    let vectors: Vec<Vec<f32>> = active_ids
        .iter()
        .filter_map(|id| stored.get(*id).cloned())
        .collect();
    let fresh = cluster::cluster(&vectors);
    if let Some(c) = &fresh {
        db.cluster_put(n, c)?;
    }
    Ok((fresh, true))
}

// ───────────────────────────────────────────────────────────── source 조립 (§6.2)

/// 붙여넣은 텍스트. `origin`은 `clipboard:<sha256 앞 12자>`.
fn text_source(text: &str) -> Source {
    let sha = sha256_hex(text.as_bytes());
    Source {
        kind: Kind::Text,
        origin: format!("clipboard:{}", short(&sha)),
        sha256: sha,
        // `bytes`는 언제나 **sha256을 계산한 바이트 수**다. 셋(파일·텍스트·URL)이
        // 같은 규칙을 쓰므로 두 값이 서로를 설명한다.
        bytes: text.len() as u64,
        mime: "text/plain".to_string(),
    }
}

/// 로컬 파일. `origin`은 `file://` **절대경로**, sha256은 파일 내용 (§6.2).
fn file_source(path: &Path, kind: Kind, mime: String) -> Result<Source> {
    Ok(Source {
        kind,
        sha256: sha256_file(path)?,
        origin: file_origin(path),
        bytes: std::fs::metadata(path)?.len(),
        mime,
    })
}

/// YouTube. `origin`은 정규형 watch URL, sha256은 **그 URL의 sha256**이다 (§6.2).
fn youtube_source(kind: Kind, meta: &YoutubeMeta) -> Source {
    let url = meta.canonical_url.clone();
    Source {
        kind,
        sha256: sha256_hex(url.as_bytes()),
        bytes: url.len() as u64,
        // 원본 파일이 없으므로 등록된 mime이 없다. kind를 잃지 않게 kind를 실어 둔다.
        mime: format!("{kind}/youtube"),
        origin: url,
    }
}

// ─────────────────────────────────────────────────────────────────── 도우미

fn note(progress: Option<ProgressFn<'_>>, stage: &str) {
    if let Some(f) = progress {
        f(stage);
    }
}

pub(crate) fn embedder<'a>(
    app: &'a crate::App,
    client: &'a soul_net::OpenAi,
) -> soul_net::embed::Embedder<'a> {
    soul_net::embed::Embedder {
        db: &app.db,
        client: Some(client),
        // §R3 — `soul-core::rebuild::EMBED_PROVIDER`와 반드시 같아야 캐시가 맞는다.
        provider: soul_core::rebuild::EMBED_PROVIDER.to_string(),
        model: app.config.embed.model.clone(),
        dims: app.config.embed.dims,
        offline: false,
    }
}

fn require_tools(app: &crate::App) -> Result<soul_media::ffmpeg::FfmpegTools> {
    soul_media::ffmpeg::locate(&app.paths.bin()).ok_or_else(|| {
        SoulError::config("ffmpeg/ffprobe를 찾을 수 없습니다. `soul doctor`로 조달하세요 (§9.7)")
    })
}

/// 썸네일 실패는 투입을 실패시키지 않는다 — 아카이브 표시용일 뿐이다 (§20.4).
fn write_thumb(app: &crate::App, sha256_hex: &str, image: &[u8]) -> Option<PathBuf> {
    match soul_media::thumb::write(
        &app.paths,
        sha256_hex,
        image,
        app.config.local.thumb_max_edge_px,
    ) {
        Ok(p) => Some(p),
        Err(e) => {
            trace_error(app, "thumbnail", e.to_string());
            None
        }
    }
}

/// T11e — 로컬 사본은 **1회만** 내려받는다. 이미 있으면 네트워크를 쓰지 않는다.
async fn youtube_thumbnail(
    app: &crate::App,
    sha256_hex: &str,
    meta: &YoutubeMeta,
) -> Option<PathBuf> {
    if soul_media::thumb::exists(&app.paths, sha256_hex) {
        return Some(app.paths.thumb_file(sha256_hex));
    }
    if meta.thumbnail_url.is_empty() {
        return None;
    }
    match youtube::fetch_thumbnail(&meta.thumbnail_url, app.config.api.timeout_secs).await {
        Ok(bytes) => write_thumb(app, sha256_hex, &bytes),
        Err(e) => {
            trace_error(app, "thumbnail", e.to_string());
            None
        }
    }
}

/// §9.5 — 오디오·최소 경로에 동봉하는 메타데이터. 출력은 입력만으로 정해진다.
fn youtube_metadata_text(meta: &YoutubeMeta) -> String {
    let mut lines = vec![format!("URL: {}", meta.canonical_url)];
    if let Some(t) = trimmed(meta.title.as_deref().unwrap_or_default()) {
        lines.push(format!("제목: {t}"));
    }
    if let Some(c) = trimmed(meta.channel.as_deref().unwrap_or_default()) {
        lines.push(format!("채널: {c}"));
    }
    if let Some(d) = meta.duration_secs {
        lines.push(format!("길이: {:.0}초", d));
    }
    if !meta.tags.is_empty() {
        lines.push(format!("태그: {}", meta.tags.join(", ")));
    }
    if let Some(d) = trimmed(meta.description.as_deref().unwrap_or_default()) {
        // 설명이 수천 자인 채널이 흔하다. 서술 프롬프트를 덮지 않게 자른다.
        let head: String = d.chars().take(1000).collect();
        lines.push(format!("설명: {head}"));
    }
    lines.join("\n")
}

fn trimmed(s: &str) -> Option<&str> {
    let t = s.trim();
    (!t.is_empty()).then_some(t)
}

fn trace_error(app: &crate::App, purpose: &str, message: String) {
    let Some(tracer) = &app.tracer else { return };
    // 트레이스 실패로 투입을 멈추지 않는다 — 재현성의 일부가 아니다 (§11.3).
    let _ = tracer.write(&soul_core::trace::TraceEntry {
        ts: Ts::now(),
        purpose: purpose.to_string(),
        model: String::new(),
        prompt_sha256: None,
        tokens_in: 0,
        tokens_out: 0,
        cost_usd_est: 0.0,
        latency_ms: 0,
        response_raw: None,
        error: Some(message),
    });
}

fn short(sha: &str) -> &str {
    &sha[..12.min(sha.len())]
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// 파일 내용의 sha256. 영상이 수 GB일 수 있으므로 통째로 읽지 않는다.
fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// `file://` + 절대경로 (§6.2). 원본을 복사하지 않으므로 이 문자열이 유일한 연결고리다 (§20.4).
fn file_origin(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    if s.starts_with('/') {
        format!("file://{s}")
    } else {
        // 윈도우 드라이브 경로: `C:/x` → `file:///C:/x`
        format!("file:///{s}")
    }
}

/// `file_origin`의 역이다. `file://`가 아니면 `None`.
fn origin_to_path(origin: &str) -> Option<PathBuf> {
    let rest = origin.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    // `/C:/x` → `C:/x`. 유닉스 경로는 그대로 둔다.
    if bytes.first() == Some(&b'/') && bytes.get(2) == Some(&b':') {
        return Some(PathBuf::from(&rest[1..]));
    }
    Some(PathBuf::from(rest))
}

// ───────────────────────────────────────────────────────────────────── 테스트

/// 오프라인 픽스처. `reading`·`recast` 테스트도 이것을 쓴다.
#[cfg(test)]
pub(crate) mod testkit {
    use super::*;
    use soul_core::config::Config;
    use soul_core::obs::{Axes, ContextObs, Quality, SourceRef, Store};
    use soul_core::paths::Paths;

    /// drop 시 루트를 지운다. `tempfile`을 새로 의존하지 않으려고 직접 만든다.
    pub(crate) struct TempApp {
        pub app: crate::App,
    }

    impl Drop for TempApp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.app.paths.root);
        }
    }

    impl std::ops::Deref for TempApp {
        type Target = crate::App;
        fn deref(&self) -> &crate::App {
            &self.app
        }
    }

    pub(crate) fn temp_app(tag: &str) -> TempApp {
        let root = std::env::temp_dir().join(format!(
            "tasty-soul-pipeline-{tag}-{}",
            soul_core::ids::new_id()
        ));
        let paths = Paths::at(root);
        paths.ensure_dirs().expect("디렉토리 생성");
        git::ensure_repo(&paths.soul()).expect("git init");
        let db = Db::open(&paths.derived_db()).expect("derived.sqlite");
        let store = Store::new(paths.clone());
        TempApp {
            app: crate::App {
                paths,
                config: Config::default(),
                db,
                store,
                tracer: None,
                // 이 테스트 하네스는 `open_or_init`을 거치지 않으므로 스켈레톤도 만들지 않았다.
                created_soul_md: false,
            },
        }
    }

    pub(crate) fn machine(prose: &str) -> Machine {
        Machine {
            prose: prose.to_string(),
            axes: Axes::from_array([0.5; 8]),
            tags: vec!["실내".into()],
            quality: Quality::Full,
            prompt_sha256: "p0".into(),
        }
    }

    pub(crate) fn model() -> ModelRef {
        ModelRef {
            provider: "openai".into(),
            id: "gpt-test".into(),
            prompt_sha256: None,
            calls: vec!["resp_1".into()],
        }
    }

    pub(crate) fn frozen(min_dist: Option<f64>, surprisal: f64) -> Surprisal {
        Surprisal {
            min_dist,
            surprisal,
        }
    }

    /// 텍스트 1건을 기록한다. 네트워크를 쓰지 않는다.
    pub(crate) fn record_text(app: &crate::App, text: &str, supersedes: Option<ObsId>) -> ObsId {
        let (id, _) = record_ingest(
            app,
            text_source(text),
            machine(text),
            model(),
            frozen(None, 1.0),
            &[0.5f32; 4],
            supersedes,
        )
        .expect("기록");
        id
    }

    /// §9.3 형태의 YouTube `ingest` 1건.
    pub(crate) fn record_youtube(app: &crate::App, kind: Kind, video_id: &str) -> ObsId {
        let meta = YoutubeMeta {
            video_id: video_id.to_string(),
            canonical_url: probe::youtube_canonical_url(video_id),
            ..Default::default()
        };
        let (id, _) = record_ingest(
            app,
            youtube_source(kind, &meta),
            machine("영상 서술"),
            model(),
            frozen(None, 1.0),
            &[0.5f32; 4],
            None,
        )
        .expect("기록");
        id
    }

    /// `IngestOutcome`은 `Debug`가 아니므로 `unwrap_err`을 쓸 수 없다.
    pub(crate) fn err_of(r: Result<IngestOutcome>) -> String {
        match r {
            Ok(o) => panic!("에러를 기대했는데 관측 {}이(가) 생겼다", o.id),
            Err(e) => e.to_string(),
        }
    }

    /// `context` 관측을 직접 써 넣는다 (비평 워커는 다른 모듈의 몫이다).
    pub(crate) fn write_context(
        app: &crate::App,
        target: &ObsId,
        critique: &str,
        sources: usize,
    ) -> ObsId {
        let (id, ts, schema) = soul_core::obs::new_header();
        let sources: Vec<SourceRef> = (0..sources)
            .map(|i| SourceRef {
                url: format!("https://example.test/{i}"),
                title: format!("근거 {i}"),
                fetched_at: ts,
            })
            .collect();
        let obs = Observation::Context(ContextObs {
            id: id.clone(),
            ts,
            schema,
            target: target.clone(),
            critique: critique.to_string(),
            lineage: vec!["슈게이즈".into()],
            queries: vec!["질의".into()],
            // §6.4 — grounded는 모델이 아니라 파이프라인이 센다.
            grounded: sources.len() >= 2,
            sources,
            model: model(),
        });
        let _lock = WriteLock::acquire(&app.paths.soul()).expect("쓰기 락");
        let path = app.store.append(&obs).expect("기록");
        git::commit_paths(&app.paths.soul(), &[&path], &format!("context {id}")).expect("커밋");
        id
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::*;
    use super::*;

    // ─────────────────────────────────────────────── source.origin 형식 (§6.2)

    #[test]
    fn text_origin_is_clipboard_plus_first_twelve_hex() {
        let s = text_source("차갑고 정돈된 실내");
        assert_eq!(s.kind, Kind::Text);
        assert_eq!(s.mime, "text/plain");
        assert_eq!(s.origin, format!("clipboard:{}", &s.sha256[..12]));
        assert_eq!(s.origin.len(), "clipboard:".len() + 12);
        // 한국어는 문자 수와 바이트 수가 다르다. `bytes`는 해시한 바이트 수다.
        assert_eq!(s.bytes, "차갑고 정돈된 실내".len() as u64);
        assert_ne!(s.bytes, "차갑고 정돈된 실내".chars().count() as u64);
    }

    #[test]
    fn text_sha256_is_of_the_utf8_bytes() {
        // 알려진 벡터로 못박는다 — sha256("abc").
        assert_eq!(
            text_source("abc").sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // 같은 텍스트는 같은 origin, 다른 텍스트는 다른 origin.
        assert_eq!(text_source("가").origin, text_source("가").origin);
        assert_ne!(text_source("가").origin, text_source("나").origin);
    }

    #[test]
    fn youtube_origin_is_the_canonical_watch_url_and_sha_is_of_that_url() {
        let url = probe::youtube_canonical_url("dQw4w9WgXcQ");
        let meta = YoutubeMeta {
            video_id: "dQw4w9WgXcQ".into(),
            canonical_url: url.clone(),
            ..Default::default()
        };
        let s = youtube_source(Kind::Audio, &meta);
        assert_eq!(s.origin, "https://www.youtube.com/watch?v=dQw4w9WgXcQ");
        assert_eq!(s.sha256, sha256_hex(url.as_bytes()));
        assert_eq!(s.kind, Kind::Audio);
        assert_eq!(s.mime, "audio/youtube");
        // 뒤집기(§9.3)로 kind가 바뀌어도 origin·sha256은 같다 — 같은 대상이다.
        assert_eq!(youtube_source(Kind::Video, &meta).sha256, s.sha256);
        assert_eq!(youtube_source(Kind::Video, &meta).mime, "video/youtube");
    }

    #[test]
    fn file_origin_is_absolute_and_round_trips() {
        let app = temp_app("file-origin");
        let path = app.paths.root.join("사진.jpg");
        std::fs::write(&path, b"\xff\xd8\xff\xe0jpeg-ish").unwrap();

        let s = file_source(&path, Kind::Image, "image/jpeg".into()).unwrap();
        assert!(s.origin.starts_with("file:///"), "{}", s.origin);
        assert_eq!(origin_to_path(&s.origin).unwrap(), path);
        assert_eq!(s.bytes, std::fs::metadata(&path).unwrap().len());
        assert_eq!(s.sha256, sha256_hex(b"\xff\xd8\xff\xe0jpeg-ish"));
        assert_eq!(s.sha256, sha256_file(&path).unwrap());
    }

    #[test]
    fn origin_to_path_ignores_non_file_origins() {
        assert!(origin_to_path("clipboard:abcdef012345").is_none());
        assert!(origin_to_path("https://www.youtube.com/watch?v=x").is_none());
        // 윈도우 형태도 되돌린다.
        assert_eq!(
            origin_to_path("file:///C:/x/a.jpg").unwrap(),
            PathBuf::from("C:/x/a.jpg")
        );
    }

    /// 원본 바이트에만 들어 있는 표식. 앱 디렉토리에서 이 바이트열이 나오면 사본이다.
    const MARKER: &[u8] = b"tasty-soul-original-bytes-marker";

    #[test]
    fn originals_are_never_copied_into_the_app_directory() {
        // T42 · §20.4 — 저장하는 것은 썸네일·sha256·origin 문자열뿐이다.
        let app = temp_app("no-copy");
        let outside = std::env::temp_dir().join(format!(
            "tasty-soul-source-{}.bin",
            soul_core::ids::new_id()
        ));
        // 크기로 찾으면 4096바이트 sqlite 한 페이지 같은 것에 걸린다. 내용으로 찾는다.
        let original = {
            let mut bytes = vec![7u8; 4096];
            bytes[..MARKER.len()].copy_from_slice(MARKER);
            bytes
        };
        std::fs::write(&outside, &original).unwrap();

        let source = file_source(&outside, Kind::Image, "image/png".into()).unwrap();
        record_ingest(
            &app,
            source,
            machine("바깥 파일"),
            model(),
            frozen(None, 1.0),
            &[0.1f32; 4],
            None,
        )
        .unwrap();

        let copied: Vec<PathBuf> = walk(&app.paths.root)
            .into_iter()
            .filter(|p| {
                std::fs::read(p)
                    .map(|b| b.windows(MARKER.len()).any(|w| w == MARKER))
                    .unwrap_or(false)
            })
            .collect();
        assert!(
            copied.is_empty(),
            "원본 사본이 앱 디렉토리에 생겼다: {copied:?}"
        );
        let _ = std::fs::remove_file(&outside);
    }

    // ──────────────────────────────────────────── 기록 · 커밋 · 큐 (§R8 · §9.10)

    #[test]
    fn one_ingest_writes_one_observation_and_one_commit_without_rerendering() {
        // §R8 · T21b — `ingest`는 `SOUL.md` 재렌더를 유발하지 않는다.
        let app = temp_app("commit");
        std::fs::write(app.paths.soul_md(), "# SOUL\n").unwrap();
        let before = std::fs::read(app.paths.soul_md()).unwrap();
        // 저장소 초기화 커밋은 이 검사의 대상이 아니다 — **증분**으로 센다.
        let log_before = git::log_messages(&app.paths.soul(), 100).unwrap().len();

        let id = record_text(&app, "첫 투입", None);

        let log = git::log_messages(&app.paths.soul(), 100).unwrap();
        assert_eq!(log.len(), log_before + 1, "쓰기 1회 = 커밋 1개 (§R8)");
        assert_eq!(log[0], format!("ingest {id}"));
        assert_eq!(app.store.count().unwrap(), 1);
        assert_eq!(std::fs::read(app.paths.soul_md()).unwrap(), before);
        assert!(!log.iter().any(|m| m.starts_with("render")));
    }

    #[test]
    fn queue_push_follows_context_enabled() {
        // T51b — 투입 1건 → critique_queue에 항목 1건.
        let mut app = temp_app("queue-on");
        app.app.config.thresholds.context_enabled = true;
        let id = record_text(&app, "문화 층 켜짐", None);
        assert_eq!(app.db.queue_items(None).unwrap().len(), 1);
        assert_eq!(
            app.db.queue_items(None).unwrap()[0].ingest_id,
            id.to_string()
        );
        assert_eq!(app.db.queue_pending_count().unwrap(), 1);
    }

    #[test]
    fn context_disabled_enqueues_nothing() {
        // T54 — context_enabled = false → 문화 글귀 미생성. 큐에 아무것도 넣지 않는다.
        let mut app = temp_app("queue-off");
        app.app.config.thresholds.context_enabled = false;
        let (id, queued) = record_ingest(
            &app,
            text_source("문화 층 꺼짐"),
            machine("문화 층 꺼짐"),
            model(),
            frozen(None, 1.0),
            &[0.5f32; 4],
            None,
        )
        .unwrap();
        assert!(!queued);
        assert!(app.db.queue_items(None).unwrap().is_empty());
        // 그래도 투입 자체는 유효하다.
        assert!(app.store.exists(&id));
    }

    #[test]
    fn frozen_values_land_in_the_cache_and_the_observation() {
        // §R4 — 투입 시점 값을 그대로 보관한다.
        let app = temp_app("frozen");
        let (id, _) = record_ingest(
            &app,
            text_source("동결"),
            machine("동결"),
            model(),
            frozen(Some(0.25), 0.75),
            &[1.0f32, 0.0, 0.0, 0.0],
            None,
        )
        .unwrap();

        let obs = app.store.read(&id).unwrap();
        let i = obs.as_ingest().unwrap();
        assert_eq!(i.min_dist, Some(0.25));
        assert_eq!(i.surprisal, 0.75);
        assert_eq!(app.db.min_dists_get().unwrap(), vec![0.25]);
        assert!(app
            .db
            .obs_vec_get(Space::Object, id.as_str())
            .unwrap()
            .is_some());
    }

    // ────────────────────────────────────────────────── supersedes (§R9, T16·T17)

    #[test]
    fn superseding_adds_a_file_leaves_the_old_one_untouched_and_drops_it_from_d() {
        // T16 — 기존 관측 파일 불변, 새 파일 추가. T17 — supersede된 것은 D에서 빠진다.
        let app = temp_app("supersede");
        let (old, _) = record_ingest(
            &app,
            text_source("원본"),
            machine("원본"),
            model(),
            frozen(Some(0.4), 0.9),
            &[0.5f32; 4],
            None,
        )
        .unwrap();
        let old_path = app
            .paths
            .observation_file(app.store.read(&old).unwrap().ts(), &old);
        let old_bytes = std::fs::read(&old_path).unwrap();

        let (new, _) = record_ingest(
            &app,
            text_source("원본"),
            machine("다시 본 원본"),
            model(),
            frozen(Some(0.1), 0.2),
            &[0.5f32; 4],
            Some(old.clone()),
        )
        .unwrap();

        assert_eq!(
            std::fs::read(&old_path).unwrap(),
            old_bytes,
            "관측은 불변이다"
        );
        assert_eq!(app.store.count().unwrap(), 2);
        let obs = app.store.read(&new).unwrap();
        assert_eq!(obs.as_ingest().unwrap().supersedes, Some(old.clone()));

        // §R9 — 파생 계산의 유일한 관문.
        let set = app.store.load_set().unwrap();
        let active: Vec<&ObsId> = set.active_ingests().iter().map(|i| &i.id).collect();
        assert_eq!(active, vec![&new]);
        assert_eq!(
            app.db.min_dists_get().unwrap(),
            vec![0.1],
            "옛 값은 D에서 빠진다"
        );
    }

    #[test]
    fn supersedes_key_is_absent_when_not_reanalyzed() {
        // §6.2 — 그 외에는 **키를 넣지 않는다**.
        let app = temp_app("no-supersedes");
        let id = record_text(&app, "보통 투입", None);
        let ts = app.store.read(&id).unwrap().ts();
        let json = std::fs::read_to_string(app.paths.observation_file(ts, &id)).unwrap();
        assert!(!json.contains("supersedes"), "{json}");
    }

    // ────────────────────────────────────────────────── 군집 캐시 (§12.3, T46)

    #[test]
    fn cached_centroids_are_reused_until_the_growth_rule_fires() {
        // T46 — 연속 투입 20건 중 군집 재계산이 2회 이하.
        let db = Db::open_in_memory().unwrap();
        let ids: Vec<String> = (0..120).map(|i| format!("01{i:024}")).collect();
        for (i, id) in ids.iter().enumerate() {
            let a = (i % 7) as f32 * 0.1;
            db.obs_vec_put(Space::Object, id, &[a, 1.0 - a, 0.2, 0.3])
                .unwrap();
        }

        // 관측 100건 상태에서 캐시를 한 번 만든다.
        let first: Vec<&str> = ids[..100].iter().map(|s| s.as_str()).collect();
        let (c, recomputed) = clustering_for(&db, &first).unwrap();
        assert!(recomputed, "캐시가 비어 있으면 만든다");
        assert!(c.is_some(), "n = 100 이면 군집이 나온다");

        let mut recomputes = 0;
        for n in 101..=120 {
            let slice: Vec<&str> = ids[..n].iter().map(|s| s.as_str()).collect();
            let (c, again) = clustering_for(&db, &slice).unwrap();
            assert!(c.is_some());
            if again {
                recomputes += 1;
            }
        }
        assert!(recomputes <= 2, "20건 동안 {recomputes}회 재계산했다");
    }

    #[test]
    fn clustering_ignores_vectors_that_are_not_active_ingests() {
        // §12.7 · T49 — `obs_vec`에는 감각 reading의 정정문도 들어 있다.
        //               활성 ingest만 군집에 들어가야 한다.
        let db = Db::open_in_memory().unwrap();
        for i in 0..6 {
            db.obs_vec_put(Space::Object, &format!("01ING{i}"), &[1.0, 0.0])
                .unwrap();
        }
        for i in 0..40 {
            // 반대편에 몰린 정정문 벡터. 들어가면 중심이 통째로 끌려간다.
            db.obs_vec_put(Space::Object, &format!("01READ{i}"), &[-1.0, 0.0])
                .unwrap();
        }
        let active: Vec<String> = (0..6).map(|i| format!("01ING{i}")).collect();
        let ids: Vec<&str> = active.iter().map(|s| s.as_str()).collect();
        let (c, _) = clustering_for(&db, &ids).unwrap();
        let c = c.expect("n = 6 ≥ 4");
        assert_eq!(c.assignment.len(), 6, "정정문 벡터가 섞여 들어왔다");
        for centroid in &c.centroids {
            assert!(
                centroid[0] > 0.0,
                "중심이 정정문 쪽으로 끌려갔다: {centroid:?}"
            );
        }
    }

    #[test]
    fn fewer_than_four_ingests_have_no_clustering() {
        // §12.3 — n < 4 이면 군집이 없다 → min_dist = null (§12.4).
        let db = Db::open_in_memory().unwrap();
        for i in 0..3 {
            db.obs_vec_put(Space::Object, &format!("01A{i}"), &[1.0, 0.0])
                .unwrap();
        }
        let ids = ["01A0", "01A1", "01A2"];
        let (c, _) = clustering_for(&db, &ids).unwrap();
        assert!(c.is_none());
        let s = surprisal::compute(&[1.0, 0.0], c.as_ref(), &[]);
        assert_eq!(s.min_dist, None);
        assert_eq!(s.surprisal, 1.0);
    }

    #[test]
    fn compute_frozen_uses_active_ingests_only() {
        let app = temp_app("frozen-active");
        // 활성 ingest 5건 + supersede된 1건.
        for i in 0..5 {
            record_ingest(
                &app,
                text_source(&format!("투입 {i}")),
                machine(&format!("투입 {i}")),
                model(),
                frozen(Some(0.3), 1.0),
                &[1.0f32, 0.0],
                None,
            )
            .unwrap();
        }
        let set = app.store.load_set().unwrap();
        let s = compute_frozen(&app, &[1.0f32, 0.0], &set).unwrap();
        assert!(s.min_dist.is_some(), "5건이면 군집이 있다");
        // |D| = 5 < 10 → surprisal = 1.0 (T18의 나눗셈 회피 경로).
        assert_eq!(s.surprisal, 1.0);
    }

    // ────────────────────────────────────────────────────── reanalyze (§20.4)

    #[tokio::test]
    async fn reanalyze_refuses_clipboard_text() {
        // §20.4 — 원본이 없으면 **명확히 알리고 중단한다**.
        let app = temp_app("reanalyze-text");
        let id = record_text(&app, "붙여넣은 글", None);
        let err = err_of(reanalyze(&app, &id).await);
        assert!(err.contains("재분석할 수 없습니다"), "{err}");
        assert_eq!(app.store.count().unwrap(), 1, "관측을 만들면 안 된다");
    }

    #[tokio::test]
    async fn reanalyze_refuses_a_missing_or_changed_original() {
        let app = temp_app("reanalyze-file");
        let path = app.paths.root.join("원본.png");
        std::fs::write(&path, b"\x89PNG\r\n\x1a\nfirst").unwrap();
        let source = file_source(&path, Kind::Image, "image/png".into()).unwrap();
        let (id, _) = record_ingest(
            &app,
            source,
            machine("이미지"),
            model(),
            frozen(None, 1.0),
            &[0.5f32; 4],
            None,
        )
        .unwrap();

        // 내용이 바뀌면 sha256이 어긋난다.
        std::fs::write(&path, b"\x89PNG\r\n\x1a\nchanged").unwrap();
        let err = err_of(reanalyze(&app, &id).await);
        assert!(err.contains("sha256"), "{err}");

        // 파일이 사라지면 그 사실을 알린다.
        std::fs::remove_file(&path).unwrap();
        let err = err_of(reanalyze(&app, &id).await);
        assert!(err.contains("원본 파일이 없어"), "{err}");
        assert_eq!(app.store.count().unwrap(), 1);
    }

    #[tokio::test]
    async fn plan_from_source_accepts_an_unchanged_original_and_reuses_the_recorded_kind() {
        // §20.4 — 바이트가 같으면 다시 판별하지 않는다. 기록된 kind·mime을 그대로 쓴다.
        let app = temp_app("reanalyze-ok");
        let path = app.paths.root.join("그대로.png");
        std::fs::write(&path, b"\x89PNG\r\n\x1a\nsame").unwrap();
        // magic bytes로는 PNG지만, 기록된 값을 쓰는지 보려고 일부러 다른 mime을 넣는다.
        let source = file_source(&path, Kind::Image, "image/heic".into()).unwrap();

        match plan_from_source(&app, &source).await.unwrap() {
            Plan::File {
                path: p,
                kind,
                mime,
            } => {
                assert_eq!(p, path);
                assert_eq!(kind, Kind::Image);
                assert_eq!(mime, "image/heic", "판별을 다시 돌리면 안 된다");
            }
            _ => panic!("파일 계획이 나와야 한다"),
        }
    }

    #[tokio::test]
    async fn reanalyze_rejects_non_ingest_targets() {
        let app = temp_app("reanalyze-kind");
        let id = record_text(&app, "투입", None);
        let reading = crate::reading::record_reading(
            &app,
            soul_core::obs::Layer::Sensory,
            &id,
            soul_core::obs::Verdict::Yes,
            None,
            None,
            None,
        )
        .unwrap();
        let err = err_of(reanalyze(&app, &reading).await);
        assert!(err.contains("ingest 관측이 아닙니다"), "{err}");
    }

    // ───────────────────────────────────────────────────────────── 메타데이터

    #[test]
    fn youtube_metadata_is_deterministic_and_skips_empty_fields() {
        let meta = YoutubeMeta {
            video_id: "abc".into(),
            canonical_url: "https://www.youtube.com/watch?v=abc".into(),
            title: Some("  제목  ".into()),
            channel: Some("   ".into()),
            duration_secs: Some(212.4),
            tags: vec!["슈게이즈".into(), "드림 팝".into()],
            ..Default::default()
        };
        let text = youtube_metadata_text(&meta);
        assert_eq!(text, youtube_metadata_text(&meta));
        assert!(text.contains("제목: 제목"));
        assert!(!text.contains("채널"), "빈 필드는 넣지 않는다: {text}");
        assert!(text.contains("길이: 212초"));
        assert!(text.contains("태그: 슈게이즈, 드림 팝"));
    }

    // ───────────────────────────────────────────── 아카이브용 로컬 사본 (§20.4, T11e)

    #[tokio::test]
    async fn the_youtube_thumbnail_copy_is_fetched_at_most_once() {
        // T11e — 서술용으로는 URL을 그대로 넘기고, 아카이브 사본은 **1회만** 내려받는다.
        //        이미 있으면 네트워크를 건드리지 않는다 (그래서 이 테스트가 오프라인이다).
        let app = temp_app("thumb-once");
        let meta = YoutubeMeta {
            video_id: "dQw4w9WgXcQ".into(),
            canonical_url: probe::youtube_canonical_url("dQw4w9WgXcQ"),
            // 닿으면 실패한다. 캐시를 보지 않는 회귀가 생기면 여기서 잡힌다.
            thumbnail_url: "http://127.0.0.1:9/절대-닿지-않는다.jpg".into(),
            ..Default::default()
        };
        let source = youtube_source(Kind::Video, &meta);

        // 첫 투입이 이미 사본을 만들어 둔 상태.
        let expected =
            soul_media::thumb::write(&app.paths, &source.sha256, &tiny_png(), 64).unwrap();

        let got = youtube_thumbnail(&app, &source.sha256, &meta).await;
        assert_eq!(got.as_deref(), Some(expected.as_path()));
        assert_eq!(got.unwrap(), app.paths.thumb_file(&source.sha256));
    }

    #[tokio::test]
    async fn a_youtube_item_without_a_thumbnail_url_gets_no_local_copy() {
        // 썸네일 실패는 투입을 실패시키지 않는다 (§20.4). URL이 없으면 조용히 없다.
        let app = temp_app("thumb-none");
        let meta = YoutubeMeta::default();
        let sha = "ab".repeat(32);
        assert!(youtube_thumbnail(&app, &sha, &meta).await.is_none());
        assert!(!soul_media::thumb::exists(&app.paths, &sha));
    }

    // ───────────────────────────────────────────── 네트워크 경로 (§9.1 전체 흐름)

    /// §9.1의 처리 흐름을 실제 API로 한 바퀴 돈다. `SOUL_E2E=1`일 때만 실행한다.
    #[tokio::test]
    #[ignore = "실제 API를 호출한다. SOUL_E2E=1 로만 실행"]
    async fn e2e_text_ingest_walks_the_whole_flow() {
        if std::env::var("SOUL_E2E").ok().as_deref() != Some("1") {
            return;
        }
        // 키는 OS 키체인에서 오므로(§2) 임시 루트로도 실제 호출이 된다.
        // 사용자의 진짜 관측 로그를 건드리지 않으려고 임시 루트를 쓴다.
        let app = temp_app("e2e-text");
        let stages = std::sync::Mutex::new(Vec::<String>::new());
        let progress = |s: &str| stages.lock().unwrap().push(s.to_string());

        let out = ingest(
            &app,
            IngestInput::Text("차갑고 정돈된, 사람이 방금 지워진 실내".into()),
            Some(&progress),
        )
        .await
        .expect("텍스트 투입");

        assert_eq!(out.kind, Kind::Text);
        assert!(!out.prose.is_empty());
        // §20.4 — text는 썸네일이 없다.
        assert!(out.thumbnail.is_none());
        // 추정한 kind가 아니다 — 뒤집기 UI를 띄우면 안 된다 (§9.3).
        assert!(!out.kind_is_guess);

        let seen = stages.lock().unwrap().clone();
        for want in [stage::DETECT, stage::DESCRIBE, stage::EMBED, stage::RECORD] {
            assert!(
                seen.iter().any(|s| s == want),
                "{want} 단계가 없다: {seen:?}"
            );
        }
        // §R8 — 쓰기 1회 = 커밋 1개.
        let log = git::log_messages(&app.paths.soul(), 10).unwrap();
        assert_eq!(log[0], format!("ingest {}", out.id));
        // §R4 — 동결값이 관측과 캐시 양쪽에 있다.
        let obs = app.store.read(&out.id).unwrap();
        assert_eq!(obs.as_ingest().unwrap().surprisal, 1.0, "첫 투입은 |D| = 0");
    }

    // ─────────────────────────────────────────────────────────────── 도우미

    /// 썸네일 원본으로 쓸 최소 PNG. 픽스처 파일을 두지 않으려고 그 자리에서 만든다.
    fn tiny_png() -> Vec<u8> {
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            8,
            8,
            image::Rgb([1, 2, 3]),
        ));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(dir) else {
            return out;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
        out
    }
}
