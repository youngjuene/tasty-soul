//! `soul doctor` (§9.9 · §14).
//!
//! 모델 ID 기본값이 빈 문자열이므로 **최초 실행 시 앱은 아무 투입도 처리할 수 없다.**
//! 이 상태를 막다른 길로 두지 않는다.
//!
//! 1. `GET {base_url}/models`로 계정에서 사용 가능한 모델 목록을 가져온다
//! 2. 네 슬롯 각각에 대해 후보를 드롭다운으로 제시한다
//! 3. 사용자가 고르면 슬롯별로 최소 호출을 날려 **실제 modality 지원 여부**를 검증한다
//!    - `vision`: 1×1 픽셀 이미지로 Responses API 호출
//!    - `audio`: 0.1초 무음 mp3로 Chat Completions `input_audio` 호출
//!    - `text`/`reflect`: 한 단어 텍스트 호출
//!    - `vision` 다중 이미지: 1×1 픽셀 `video_max_frames`장을 **한 호출**에 넣어 거부되지 않는지
//!    - `embed`: 한 단어 임베딩 후 반환 차원이 `embed.dims`와 일치하는지.
//!      모델이 `dimensions` 파라미터를 지원하지 않으면 **실패로 처리한다**
//! 4. 실패한 슬롯은 사유와 함께 표시하고 **해당 투입 경로만** 비활성화한다
//!
//! 목록 조회가 실패하면 자유 입력 필드로 폴백한다.

use base64::prelude::{Engine as _, BASE64_STANDARD};
use soul_core::config::ModelsConfig;
use soul_core::error::{Result, SoulError};
use soul_net::openai::Part;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DoctorReport {
    pub api_key_set: bool,
    pub models_available: Vec<String>,
    pub slots: Vec<SlotCheck>,
    pub embed_ok: Option<bool>,
    pub embed_error: Option<String>,
    pub ffmpeg: Option<String>,
    pub ffprobe: Option<String>,
    pub ytdlp: Option<String>,
    pub git_ok: bool,
    pub soul_md_ok: bool,
    /// §9.6 토큰 주의 — 30장 호출이 가능한가.
    pub multi_image_ok: Option<bool>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SlotCheck {
    pub slot: String,
    pub model: String,
    pub ok: bool,
    pub error: Option<String>,
}

/// 진단 호출의 시스템 지시. **§R11의 프롬프트가 아니다** — 관측을 만들지 않으므로
/// `prompt_sha256`에 걸리지 않는다. 모달리티가 살아 있는지만 보는 최소 페이로드다.
const PROBE_SYSTEM: &str = "한 단어로만 답한다.";
const PROBE_INPUT: &str = "안녕";

/// 슬롯이 비어 있을 때의 사유. UI가 그대로 보여준다 (§9.9 단계 4).
const UNSET_REASON: &str = "모델이 설정되지 않았습니다 (§9.9)";

pub async fn run(app: &crate::App, probe_models: bool) -> Result<DoctorReport> {
    let paths = app.paths.clone();

    let mut report = DoctorReport {
        // §2 — 값이 아니라 존재 여부만 본다.
        api_key_set: guard(|| soul_net::secrets::is_set(soul_net::secrets::ACCOUNT_OPENAI))
            .unwrap_or(false),
        models_available: Vec::new(),
        slots: Vec::new(),
        embed_ok: None,
        embed_error: None,
        ffmpeg: None,
        ffprobe: None,
        ytdlp: None,
        git_ok: false,
        soul_md_ok: false,
        multi_image_ok: None,
    };

    // ── 로컬 점검. 여기까지는 네트워크를 전혀 쓰지 않는다.
    let bin = paths.bin();
    if let Some(Some(tools)) = guard(move || soul_media::ffmpeg::locate(&bin)) {
        report.ffmpeg = tool_version(&tools.ffmpeg, "-version");
        report.ffprobe = tool_version(&tools.ffprobe, "-version");
    }
    // yt-dlp는 §9.3 단계 5에서만 쓴다. 없어도 정상이며 메타데이터 경로로 진행한다.
    report.ytdlp = find_tool(&paths.bin(), "yt-dlp").and_then(|p| tool_version(&p, "--version"));
    // 저장소가 열리는가 (§R8). 커밋이 하나도 없어도 `Ok`다.
    report.git_ok = soul_core::git::log_messages(&paths.soul(), 1).is_ok();
    // 존재만이 아니라 §8.3 마커 짝이 맞는지까지 본다 (T10).
    report.soul_md_ok = std::fs::read_to_string(paths.soul_md())
        .ok()
        .map(|s| soul_core::soulmd::parse(&s).is_ok())
        .unwrap_or(false);

    if !probe_models {
        return Ok(report);
    }
    // §15 — 키가 없으면 모든 투입 경로가 비활성화된다. 원격 점검을 시도하지 않는다.
    if !report.api_key_set {
        return Ok(report);
    }

    let client = match app.openai() {
        Ok(c) => c,
        Err(e) => {
            report.embed_error = Some(e.to_string());
            return Ok(report);
        }
    };

    // ── 단계 1. 목록 조회. 실패해도 계속 간다 — 자유 입력 폴백은 UI가 한다 (§9.9).
    match client.list_models().await {
        Ok(v) => report.models_available = v,
        Err(e) => {
            report.models_available.clear();
            trace(app, "doctor.list_models", "", Some(e.to_string()));
        }
    }

    // ── 단계 3. 슬롯별 최소 호출.
    for slot in ModelsConfig::SLOTS {
        let model = app
            .config
            .models
            .slot(slot)
            .unwrap_or("")
            .trim()
            .to_string();
        if model.is_empty() {
            report.slots.push(SlotCheck {
                slot: slot.to_string(),
                model,
                ok: false,
                error: Some(UNSET_REASON.to_string()),
            });
            continue;
        }
        let err = probe_slot(app, &client, slot, &model).await.err();
        trace(
            app,
            &format!("doctor.{slot}"),
            &model,
            err.as_ref().map(|e| e.to_string()),
        );
        report.slots.push(SlotCheck {
            slot: slot.to_string(),
            model: model.clone(),
            ok: err.is_none(),
            error: err.map(|e| e.to_string()),
        });
    }

    // §9.6 — 1×1을 `video_max_frames`장 한 호출에. vision이 죽어 있으면 물어볼 것이 없다.
    let vision_ok = report.slots.iter().any(|s| s.slot == "vision" && s.ok);
    if vision_ok {
        let model = app.config.models.vision.trim().to_string();
        let n = app.config.thresholds.video_max_frames as usize;
        let err = probe_multi_image(&client, &model, n).await.err();
        trace(
            app,
            "doctor.multi_image",
            &model,
            err.as_ref().map(|e| e.to_string()),
        );
        report.multi_image_ok = Some(err.is_none());
    }

    // ── `embed` 슬롯. 차원이 어긋나면 §20.2의 전제가 무너진다.
    let (ok, err) = probe_embed(app, &client).await;
    report.embed_ok = Some(ok);
    report.embed_error = err;

    Ok(report)
}

// ─────────────────────────────────────────────────────────────── 슬롯 검증

async fn probe_slot(
    app: &crate::App,
    client: &soul_net::OpenAi,
    slot: &str,
    model: &str,
) -> Result<()> {
    let schema = probe_schema();
    match slot {
        // Responses API, 이미지 입력 (§9.4).
        "vision" => {
            client
                .responses_json(
                    model,
                    PROBE_SYSTEM,
                    vec![
                        Part::ImageUrl(one_by_one_png_data_url()?),
                        Part::Text(PROBE_INPUT.into()),
                    ],
                    "doctor",
                    &schema,
                )
                .await?;
        }
        // Chat Completions `input_audio` (§9.5). Responses API는 오디오를 받지 않는다.
        "audio" => {
            client
                .audio_json(
                    model,
                    PROBE_SYSTEM,
                    vec![PROBE_INPUT.to_string()],
                    &silent_mp3_100ms(),
                    "doctor",
                    &schema,
                )
                .await?;
        }
        // 텍스트·병합(§10.3)과 성찰 에이전트(§10.5).
        "text" | "reflect" => {
            client
                .responses_json(
                    model,
                    PROBE_SYSTEM,
                    vec![Part::Text(PROBE_INPUT.into())],
                    "doctor",
                    &schema,
                )
                .await?;
        }
        other => {
            return Err(SoulError::invalid(format!("알 수 없는 슬롯: {other}")));
        }
    }
    let _ = app;
    Ok(())
}

/// §9.6 — 프레임 전체를 한 호출에 넣는 것이 거부되지 않는지.
/// 거부되면 `video_max_frames`를 낮추도록 안내한다 (권장: 15).
async fn probe_multi_image(client: &soul_net::OpenAi, model: &str, frames: usize) -> Result<()> {
    let url = one_by_one_png_data_url()?;
    let mut parts: Vec<Part> = (0..frames.max(1))
        .map(|_| Part::ImageUrl(url.clone()))
        .collect();
    parts.push(Part::Text(PROBE_INPUT.to_string()));
    client
        .responses_json(model, PROBE_SYSTEM, parts, "doctor", &probe_schema())
        .await?;
    Ok(())
}

/// §9.9 — 반환 차원이 `embed.dims`와 일치해야 한다.
/// **`dimensions`를 지원하지 않는 모델은 실패다** — 지원하지 않으면 1536차원이 그대로 와서
/// §20.2의 저장·계산 예산이 6배로 늘고 캐시(§R3)도 전부 어긋난다.
async fn probe_embed(app: &crate::App, client: &soul_net::OpenAi) -> (bool, Option<String>) {
    let model = app.config.embed.model.trim().to_string();
    if model.is_empty() {
        return (false, Some(UNSET_REASON.to_string()));
    }
    let dims = app.config.embed.dims;
    match client.embed(&model, dims, &[PROBE_INPUT.to_string()]).await {
        Ok(v) => {
            let got = v.first().map(|x| x.len()).unwrap_or(0);
            trace(app, "doctor.embed", &model, None);
            if got == dims {
                (true, None)
            } else {
                (
                    false,
                    Some(format!(
                        "임베딩이 {got}차원으로 왔습니다. embed.dims={dims}와 다릅니다 — \
                         `dimensions` 파라미터를 지원하지 않는 모델은 쓸 수 없습니다 (§9.9·§20.2)"
                    )),
                )
            }
        }
        Err(e) => {
            trace(app, "doctor.embed", &model, Some(e.to_string()));
            (false, Some(e.to_string()))
        }
    }
}

// ───────────────────────────────────────────────────────────── 검증 페이로드

/// 진단용 최소 스키마. §10의 서술 스키마를 쓰면 답을 만드느라 토큰이 낭비된다.
fn probe_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["ok"],
        "properties": { "ok": { "type": "string" } }
    })
}

/// §9.9 — 1×1 픽셀 PNG data URL. 비전 슬롯 검증용 최소 이미지.
pub fn one_by_one_png_data_url() -> Result<String> {
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::new(1, 1));
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| SoulError::invalid(format!("1×1 PNG를 만들지 못했습니다: {e}")))?;
    Ok(format!(
        "data:image/png;base64,{}",
        BASE64_STANDARD.encode(buf.get_ref())
    ))
}

/// §9.9 — 0.1초 무음 mp3. **ffmpeg에 기대지 않는다** — ffmpeg이 없는 것도 doctor가
/// 진단하려는 상태이므로, 그 상태에서 오디오 슬롯을 못 보면 진단이 반쪽이 된다.
///
/// MPEG-1 Layer III · 44.1 kHz · 32 kbps · mono → 프레임 하나가 1152샘플(26.12 ms)·104 바이트다.
/// side info와 main data가 전부 0이면 디코더는 무음을 낸다.
pub fn silent_mp3_100ms() -> Vec<u8> {
    // FF FB = sync + MPEG1 + Layer III + CRC 없음, 10 = 32 kbps/44.1 kHz, C4 = mono.
    const HEADER: [u8; 4] = [0xFF, 0xFB, 0x10, 0xC4];
    const FRAME_BYTES: usize = 104;
    const FRAMES: usize = 4; // 4 × 26.12 ms ≈ 104 ms

    let mut out = Vec::with_capacity(FRAME_BYTES * FRAMES);
    for _ in 0..FRAMES {
        out.extend_from_slice(&HEADER);
        out.resize(out.len() + FRAME_BYTES - HEADER.len(), 0);
    }
    out
}

// ─────────────────────────────────────────────────────────────── 로컬 점검

/// **`soul doctor`는 무언가 고장났을 때 실행하는 도구다.** 개별 점검이 패닉하더라도
/// 나머지 진단 결과는 사용자에게 돌아가야 한다. 여기서 삼키는 것은 진단 항목 하나뿐이고,
/// 그 항목은 "확인 불가"(`None`)로 보고된다.
fn guard<T>(f: impl FnOnce() -> T + std::panic::UnwindSafe) -> Option<T> {
    std::panic::catch_unwind(f).ok()
}

/// `<root>/bin/` → `PATH` 순으로 찾는다 (§9.7과 같은 순서).
fn find_tool(bin_dir: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
    let exe = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    let local = bin_dir.join(&exe);
    if local.is_file() {
        return Some(local);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(&exe))
        .find(|p| p.is_file())
}

/// `<tool> <flag>`의 첫 줄. 실행하지 못하면 `None`.
fn tool_version(path: &std::path::Path, flag: &str) -> Option<String> {
    let out = std::process::Command::new(path).arg(flag).output().ok()?;
    if !out.status.success() && out.stdout.is_empty() {
        return None;
    }
    let text = if out.stdout.is_empty() {
        String::from_utf8_lossy(&out.stderr).to_string()
    } else {
        String::from_utf8_lossy(&out.stdout).to_string()
    };
    text.lines()
        .next()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
}

/// 진단 호출도 트레이스에 남긴다 (§11.3). 응답 본문은 담지 않는다 — 진단 페이로드다.
fn trace(app: &crate::App, purpose: &str, model: &str, error: Option<String>) {
    let Some(t) = app.tracer.as_ref() else {
        return;
    };
    let _ = t.write(&soul_core::trace::TraceEntry {
        ts: soul_core::time::Ts::now(),
        purpose: purpose.to_string(),
        model: model.to_string(),
        prompt_sha256: None,
        tokens_in: 0,
        tokens_out: 0,
        cost_usd_est: 0.0,
        latency_ms: 0,
        response_raw: None,
        error,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use soul_core::paths::Paths;

    fn temp_app() -> crate::App {
        let root = std::env::temp_dir()
            .join("tasty-soul-doctor-test")
            .join(soul_core::ids::new_id().to_string());
        crate::App::open_or_init(Paths::at(root)).unwrap()
    }

    /// §9.9 — `probe_models = false`는 **네트워크를 쓰지 않는다.**
    /// (키체인·ffmpeg 탐색이 아직 미구현이면 `guard`가 삼키고 "확인 불가"로 보고한다.)
    #[tokio::test]
    async fn local_only_run_touches_no_network() {
        let app = temp_app();
        let r = run(&app, false).await.unwrap();

        assert!(r.git_ok, "open_or_init이 만든 저장소가 열려야 한다");
        assert!(r.soul_md_ok, "스켈레톤 SOUL.md는 §8.3 파서를 통과한다");

        // 원격 점검은 하나도 하지 않았다.
        assert!(r.models_available.is_empty());
        assert!(r.slots.is_empty());
        assert_eq!(r.embed_ok, None);
        assert_eq!(r.multi_image_ok, None);
    }

    /// 깨진 `SOUL.md`는 그대로 보고한다 (T10 — 파일을 고치지 않는다).
    #[tokio::test]
    async fn broken_soul_md_is_reported() {
        let app = temp_app();
        let before = std::fs::read_to_string(app.paths.soul_md()).unwrap();
        std::fs::write(
            app.paths.soul_md(),
            "<!-- soul:gen id=header -->\n짝이 없다\n",
        )
        .unwrap();

        let r = run(&app, false).await.unwrap();
        assert!(!r.soul_md_ok);
        assert_ne!(
            std::fs::read_to_string(app.paths.soul_md()).unwrap(),
            before,
            "doctor는 진단만 한다 — 되돌려 놓지 않는다"
        );
    }

    /// §9.9 — 1×1 PNG가 실제로 1×1 PNG여야 한다.
    #[test]
    fn one_by_one_png_is_a_real_png() {
        let url = one_by_one_png_data_url().unwrap();
        let b64 = url
            .strip_prefix("data:image/png;base64,")
            .expect("data URL");
        let bytes = BASE64_STANDARD.decode(b64).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "PNG 시그니처");

        let img = image::load_from_memory(&bytes).unwrap();
        assert_eq!((img.width(), img.height()), (1, 1));
    }

    /// §9.9 — 0.1초 무음 mp3. 프레임 헤더가 유효하고 길이가 0.1초 언저리다.
    #[test]
    fn silent_mp3_frames_are_well_formed() {
        let mp3 = silent_mp3_100ms();
        assert_eq!(mp3.len(), 104 * 4);

        let mut frames = 0;
        for chunk in mp3.chunks(104) {
            assert_eq!(chunk[0], 0xFF, "sync 바이트");
            assert_eq!(chunk[1] & 0xFE, 0xFA, "MPEG-1 Layer III");
            assert!(
                chunk[4..].iter().all(|b| *b == 0),
                "무음이므로 본문은 0이다"
            );
            frames += 1;
        }
        assert_eq!(frames, 4);
        // 1152 샘플 / 44100 Hz × 4 프레임 ≈ 104 ms.
        let ms = 1152.0 / 44100.0 * 1000.0 * frames as f64;
        assert!((100.0..120.0).contains(&ms), "{ms}ms");
    }

    #[test]
    fn probe_schema_is_strict() {
        let s = probe_schema();
        assert_eq!(s["additionalProperties"], serde_json::json!(false));
        assert_eq!(s["required"], serde_json::json!(["ok"]));
    }

    #[test]
    fn find_tool_prefers_bin_dir() {
        let dir = std::env::temp_dir()
            .join("tasty-soul-doctor-bin")
            .join(soul_core::ids::new_id().to_string());
        std::fs::create_dir_all(&dir).unwrap();
        let name = if cfg!(windows) {
            "fake-tool.exe"
        } else {
            "fake-tool"
        };
        let f = dir.join(name);
        std::fs::write(&f, b"#!/bin/sh\n").unwrap();
        assert_eq!(find_tool(&dir, "fake-tool"), Some(f));
        assert_eq!(find_tool(&dir, "tasty-soul-no-such-tool"), None);
    }

    /// 슬롯이 비어 있으면 사유와 함께 실패로 보고한다 (§9.9 단계 4).
    #[test]
    fn unset_slot_reason_is_stable() {
        assert!(UNSET_REASON.contains("설정되지 않았습니다"));
    }

    /// 네트워크 경로. `SOUL_E2E=1`이고 키·모델이 설정되어 있을 때만 돈다.
    #[tokio::test]
    #[ignore = "실제 API를 호출한다. SOUL_E2E=1 로만 실행"]
    async fn e2e_probe_models() {
        if std::env::var("SOUL_E2E").ok().as_deref() != Some("1") {
            return;
        }
        let app = crate::App::open_or_init(Paths::discover().unwrap()).unwrap();
        let r = run(&app, true).await.unwrap();
        assert!(r.api_key_set);
        for s in &r.slots {
            eprintln!("{} {} ok={} {:?}", s.slot, s.model, s.ok, s.error);
        }
        eprintln!("embed_ok={:?} {:?}", r.embed_ok, r.embed_error);
    }
}
