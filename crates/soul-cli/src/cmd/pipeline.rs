//! 파이프라인을 부르는 명령들 (§14).
//!
//! `doctor` · `ingest` · `read` · `context` · `recast` · `reflect` · `reanalyze`.
//! 전부 비동기이고 `soul_pipeline::App`이 필요하다. 나머지 명령(`render`·`rebuild`·
//! `stats`·`export`·`maintain`·`trace purge`)은 `soul-core`만으로 돌기 때문에
//! 런타임도 앱 상태도 만들지 않는다.
//!
//! ## CLI가 UI를 대신하지 않는 지점
//!
//! - `reflect`는 **제안까지만** 한다. 승인은 사용자의 몫이므로(§11.2 · §13 화면 4)
//!   `soul_delta`를 여기서 기록하지 않는다.
//! - `recast`는 확인 다이얼로그(§9.3) 대신 **버려질 응답 수를 먼저 출력**한다.
//! - 카드에 답하지 않아도 투입은 유효하다 (§13 화면 2). `ingest`는 응답을 기다리지 않는다.

use anyhow::{bail, Context, Result};
use soul_core::ids::ObsId;
use soul_core::obs::{Kind, Layer, Observation, Store, Verdict};
use soul_core::paths::Paths;
use soul_pipeline::{
    critique_worker, doctor as doctor_mod, ingest as ingest_mod, reading, recast as recast_mod,
    reflect_flow, App,
};

// ─────────────────────────────────────────────────────────────── doctor

/// `soul doctor [--probe]` (§9.9).
///
/// 실패한 항목이 하나라도 있으면 **종료 코드 1**이다. 진단 명령이 언제나 0으로 끝나면
/// 최초 실행의 막다른 길(모델 ID가 전부 빈 문자열)을 CI가 잡아내지 못한다.
pub async fn doctor(paths: Paths, probe: bool) -> Result<()> {
    let app = App::open_or_init(paths)?;
    let r = doctor_mod::run(&app, probe).await?;

    let mut failed: Vec<String> = Vec::new();

    println!("API 키       {}", mark(r.api_key_set));
    if !r.api_key_set {
        failed.push("API 키 미설정 — 모든 투입 경로가 비활성화됩니다 (§15)".into());
    }
    println!("모델 목록    {}개", r.models_available.len());

    for s in &r.slots {
        println!(
            "  {:<8} {:<28} {}{}",
            s.slot,
            if s.model.is_empty() {
                "(미설정)"
            } else {
                s.model.as_str()
            },
            mark(s.ok),
            s.error
                .as_deref()
                .map(|e| format!(" — {e}"))
                .unwrap_or_default()
        );
        if !s.ok {
            failed.push(format!("모델 슬롯 {}", s.slot));
        }
    }

    println!("임베딩       {}", tri(r.embed_ok));
    if let Some(e) = &r.embed_error {
        println!("             {e}");
    }
    if r.embed_ok == Some(false) {
        failed.push("임베딩 — dimensions 미지원이면 실패로 봅니다 (§9.9)".into());
    }

    println!("다중 이미지  {}", tri(r.multi_image_ok));
    if r.multi_image_ok == Some(false) {
        failed.push("한 호출에 video_max_frames장을 넣지 못합니다 (§9.6)".into());
    }

    println!("ffmpeg       {}", opt(&r.ffmpeg));
    println!("ffprobe      {}", opt(&r.ffprobe));
    println!("yt-dlp       {}", opt(&r.ytdlp));
    println!("git          {}", mark(r.git_ok));
    println!("SOUL.md      {}", mark(r.soul_md_ok));
    if !r.git_ok {
        failed.push("git 저장소".into());
    }
    if !r.soul_md_ok {
        failed.push("SOUL.md".into());
    }

    if failed.is_empty() {
        println!("\n모든 검사를 통과했습니다.");
        return Ok(());
    }
    println!();
    for f in &failed {
        eprintln!("실패: {f}");
    }
    bail!("{}개 항목이 실패했습니다", failed.len())
}

fn mark(ok: bool) -> &'static str {
    if ok {
        "ok"
    } else {
        "실패"
    }
}

fn tri(v: Option<bool>) -> &'static str {
    match v {
        Some(true) => "ok",
        Some(false) => "실패",
        None => "미검사",
    }
}

fn opt(v: &Option<String>) -> String {
    v.clone().unwrap_or_else(|| "없음".to_string())
}

// ─────────────────────────────────────────────────────────────── ingest

/// `soul ingest <path|url|->` (§9.1).
///
/// `-`는 stdin의 텍스트다. `http(s)` URL은 YouTube 경로로 보낸다 —
/// **YouTube가 아닌 URL을 여기서 판별하지 않는다.** 거절 사유를 한 곳(§9.3)에
/// 모아 두어야 T11c의 메시지가 갈라지지 않는다.
pub async fn ingest(paths: Paths, target: String) -> Result<()> {
    use std::io::Read;

    let input = if target == "-" {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .context("stdin에서 텍스트를 읽지 못했습니다")?;
        if s.trim().is_empty() {
            bail!("stdin이 비었습니다");
        }
        ingest_mod::IngestInput::Text(s)
    } else if target.starts_with("http://") || target.starts_with("https://") {
        ingest_mod::IngestInput::Youtube {
            url: target,
            forced_kind: None,
        }
    } else {
        let p = std::path::PathBuf::from(&target);
        if !p.exists() {
            bail!(
                "{} 이 없습니다. 경로·URL 또는 `-`(stdin)를 주세요",
                p.display()
            );
        }
        ingest_mod::IngestInput::File(p)
    };

    let app = App::open_or_init(paths)?;
    // §13 화면 1 — 진행 단계를 알린다. 결과(stdout)와 섞이지 않게 stderr로 낸다.
    let progress = |step: &str| eprintln!("… {step}");
    let progress: ingest_mod::ProgressFn<'_> = &progress;
    let out = ingest_mod::ingest(&app, input, Some(progress)).await?;
    print_ingest(&out);
    Ok(())
}

/// `soul reanalyze <obs_id>` (§R9 · T16).
pub async fn reanalyze(paths: Paths, id: ObsId) -> Result<()> {
    let app = App::open_or_init(paths)?;
    let out = ingest_mod::reanalyze(&app, &id).await?;
    println!("{id} 를 재분석했습니다. 기존 관측 파일은 그대로입니다 (§R9).");
    print_ingest(&out);
    Ok(())
}

fn print_ingest(out: &ingest_mod::IngestOutcome) {
    println!(
        "{} {} ({})",
        out.id,
        out.kind,
        if out.kind_is_guess {
            "추정"
        } else {
            "확정"
        }
    );
    println!("{}", out.prose);
    if let Some(t) = &out.thumbnail {
        println!("썸네일 {}", t.display());
    }
    if out.queued_for_critique {
        println!(
            "문화 글귀를 큐에 넣었습니다 (§9.10). `soul context {}` 로 지금 만들 수 있습니다.",
            out.id
        );
    } else {
        println!("문화 글귀는 만들지 않습니다 (context_enabled = false).");
    }
    if out.kind_is_guess {
        println!("kind가 틀렸으면 `soul recast {} <kind>` (§9.3).", out.id);
    }
}

// ─────────────────────────────────────────────────────────────── read

/// `soul read <obs_id> <verdict> [prose]` (§6.3).
///
/// **`layer`를 인자로 받지 않는다.** §12.6이 이미 정해 두었기 때문이다 —
/// `ingest`를 가리키면 감각, `context`를 가리키면 문화다. 사용자가 층을 직접 고르게
/// 하면 2단 조인(T55c)이 성립하지 않는 조합을 만들 수 있다.
pub async fn read(paths: Paths, id: ObsId, verdict: Verdict, prose: Option<String>) -> Result<()> {
    // 앱을 열기 전에 대상을 확인한다 — 잘못된 ID를 빨리 알린다.
    let set = Store::new(paths.clone()).load_set()?;
    let layer = match set.get(&id) {
        Some(Observation::Ingest(_)) => Layer::Sensory,
        Some(Observation::Context(_)) => Layer::Cultural,
        Some(o) => bail!(
            "{id} 은 {} 관측입니다. ○/× 응답은 ingest(감각) 또는 context(문화)에만 답니다 (§12.6)",
            o.type_name()
        ),
        None => bail!("관측 {id} 을 찾을 수 없습니다"),
    };

    // T7 — `yes`에는 문장이 붙지 않는다. 조용히 버리지 않고 알린다.
    let prose = match (verdict, prose) {
        (Verdict::Yes, Some(_)) => {
            bail!("verdict가 yes면 문장을 받지 않습니다 (prose·divergence 모두 null, T7)")
        }
        (_, p) => p,
    };

    let app = App::open_or_init(paths)?;
    let out = reading::record(&app, &id, layer, verdict, prose).await?;
    println!(
        "{out} — {} {} 응답을 기록했습니다.",
        layer.as_str(),
        verdict.as_str()
    );
    Ok(())
}

// ─────────────────────────────────────────────────────────────── context

/// `soul context <ingest_id> [--redo]` (§9.10).
///
/// 평시엔 투입마다 자동으로 돈다. 이 명령은 실패한 항목을 다시 돌리거나(§9.10)
/// 이미 있는 글귀를 새로 만들 때(`--redo`) 쓴다. `--redo` 없이 이미 있으면
/// **아무것도 하지 않는다** — 모르고 두 번 부르면 문화 카드가 늘어난다 (T55b).
pub async fn context(paths: Paths, id: ObsId, redo: bool) -> Result<()> {
    let set = Store::new(paths.clone()).load_set()?;
    match set.get(&id) {
        Some(Observation::Ingest(_)) => {}
        Some(o) => bail!("{id} 은 {} 관측입니다. ingest ID를 주세요", o.type_name()),
        None => bail!("관측 {id} 을 찾을 수 없습니다"),
    }
    if let Some(c) = set.latest_context_for(&id) {
        if !redo {
            println!("이미 문화 글귀가 있습니다: {}", c.id);
            println!("다시 만들려면 --redo 를 주세요 (§9.10).");
            return Ok(());
        }
    }

    let app = App::open_or_init(paths)?;
    match critique_worker::run_one(&app, &id).await? {
        Some(ctx) => println!("{ctx} — 문화 글귀를 만들었습니다."),
        // T52 — 근거를 하나도 못 찾으면 관측을 만들지 않는다. 추측 서술을 넣지 않는다.
        None => println!("검색 근거를 찾지 못해 context 관측을 만들지 않았습니다 (T52)."),
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────── recast

/// `soul recast <ingest_id> <kind>` (§9.3).
pub async fn recast(paths: Paths, id: ObsId, kind: Kind) -> Result<()> {
    let app = App::open_or_init(paths)?;

    // §9.3 — 응답을 마친 뒤 뒤집으면 그 응답이 버려진다. UI는 다이얼로그로 알리고,
    // CLI는 실행 전에 수를 출력한다.
    let discarded = recast_mod::discarded_readings(&app, &id)?;
    if discarded > 0 {
        eprintln!("경고: 이 항목에 달린 응답 {discarded}건이 이월되지 않습니다 (§9.3).");
    }

    let out = recast_mod::recast(&app, &id, kind).await?;
    println!("{id} → {} (supersedes 기록, §R9)", out.id);
    print_ingest(&out);
    Ok(())
}

// ─────────────────────────────────────────────────────────────── reflect

/// `soul reflect [--force]` (§11.2).
///
/// **제안까지만 한다.** 승인은 사용자가 앱의 diff 뷰에서 한다 (§13 화면 4).
/// CLI가 자동 승인하면 `soul_delta`가 사람 확인 없이 축 `offset`을 움직인다.
pub async fn reflect(paths: Paths, force: bool) -> Result<()> {
    let app = App::open_or_init(paths)?;
    let next_path = app.paths.soul_next_md();
    let Some(p) = reflect_flow::propose(&app, force).await? else {
        println!("성찰 트리거 조건을 만족하지 않습니다 (§11.2). 지금 돌리려면 --force.");
        return Ok(());
    };

    println!(
        "제안 {} (창 {} … {})",
        p.delta.id, p.delta.window.from, p.delta.window.to
    );
    println!("근거 {}", p.delta.rationale);
    if !p.delta.cites.is_empty() {
        println!(
            "인용 {}",
            p.delta
                .cites
                .iter()
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if p.delta.axis_delta.is_empty() {
        println!("축 변화 없음");
    } else {
        for (axis, d) in &p.delta.axis_delta {
            println!("축 {axis} {}", soul_core::soulmd::fmt_change(Some(*d)));
        }
    }
    for (block, b) in &p.delta.blocks {
        println!("블록 {block} (from_hash {})", b.from_hash);
    }
    println!(
        "\n{} 에 제안을 썼습니다. 앱의 승인 화면에서 확인하세요 (§13 화면 4).",
        next_path.display()
    );
    println!(
        "길이 {} → {} 자",
        p.current_md.chars().count(),
        p.next_md.chars().count()
    );
    Ok(())
}
