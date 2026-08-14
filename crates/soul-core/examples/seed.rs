//! 관측 로그를 결정론적으로 채운다 — **개발·검증용**이다.
//!
//! ```bash
//! cargo run -p soul-core --example seed -- /tmp/soul-demo 120
//! SOUL_ROOT=/tmp/soul-demo cargo run -p soul-cli -- render
//! ```
//!
//! API 키 없이 읽기 경로 전체(파생 층 · `SOUL.md` · 아카이브 · MCP)를 시험하려면
//! 관측이 필요한데, 진짜 투입은 원격 호출을 요구한다. 그래서 §6 스키마를 만족하는
//! 관측을 직접 쓴다.
//!
//! **임베딩도 함께 채운다.** 텍스트에서 해시로 만든 가짜 벡터라 의미는 없지만,
//! 차원과 결정론이 맞으므로 §12 의 군집·drift·crystal 이 실제로 돈다.
//! 진짜 임베딩이 아니므로 이 데이터로 취향을 읽지 말 것.

use soul_core::db::embed_cache::{cache_key, Space};
use soul_core::db::Db;
use soul_core::ids::ObsId;
use soul_core::obs::*;
use soul_core::paths::Paths;
use soul_core::time::Ts;
use soul_core::{git, rebuild, vecmath, SCHEMA_VERSION};

const PROSE: [&str; 8] = [
    "차갑고 정돈된, 사람이 방금 지워진 실내",
    "젖은 아스팔트가 신호등을 두 번 삼킨다",
    "빛이 바랜 여름 마당, 아무도 부르지 않는다",
    "소리가 벽을 통과하지 못하고 방 안에서만 늙는다",
    "겨울 아침의 부엌은 아직 아무도 깨우지 않았다",
    "정적이 실내를 붙잡고 놓지 않는다",
    "저역이 방을 삼키고 천천히 놓아준다",
    "복도 끝의 빛이 바닥에만 닿는다",
];
const CRITIQUE: [&str; 3] = [
    "잔향을 악기처럼 다루던 어법을 따르되 여백을 남긴다. 소리의 크기가 아니라 사라지는 속도로 공간을 만든다.",
    "1970년대 뉴토포그래픽스의 시선을 빌린다. 사람을 지우고 남은 구조만으로 시간을 말하려 한다.",
    "미니멀리즘 실내 사진의 계보 위에 있으나, 정돈보다 방치 쪽에 무게를 둔다.",
];
const TAGS: [&str; 6] = ["실내", "자연광", "무인", "젖음", "저채도", "새벽"];

/// 텍스트에서 만드는 결정론적 가짜 임베딩. 같은 글자면 항상 같은 벡터다.
fn fake_embed(text: &str, dims: usize) -> Vec<f32> {
    use sha2::{Digest, Sha256};
    let mut out = Vec::with_capacity(dims);
    let mut counter: u32 = 0;
    while out.len() < dims {
        let mut h = Sha256::new();
        h.update(text.as_bytes());
        h.update(counter.to_le_bytes());
        for c in h.finalize().chunks_exact(4) {
            if out.len() == dims {
                break;
            }
            let v = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
            out.push((v as f64 / u32::MAX as f64) as f32 - 0.5);
        }
        counter += 1;
    }
    vecmath::normalized(&out)
}

/// ULID 를 **그 관측의 `ts` 에서** 만든다.
///
/// ULID 사전순 = 재생 순서(§6)이고 `T_ref` 는 최대 `ts`(§R1)다. 둘을 따로 만들면
/// 순서가 어긋나 재생과 시간 창이 서로 다른 이야기를 한다. 시각에서 파생시키면
/// 두 순서가 자동으로 일치한다. `seq` 는 같은 밀리초 안의 유일성만 담당한다.
fn id_at(ts: Ts, seq: u64) -> ObsId {
    let ms = ts.as_datetime().timestamp_millis().max(0) as u64;
    ObsId::parse(&ulid::Ulid::from_parts(ms, seq as u128).to_string()).expect("ULID")
}

/// `i / n` 위치를 최근 210일 구간에 대응시킨다.
///
/// **ingest 를 전 구간에 고르게 펼치는 것이 핵심이다.** 앞쪽에 몰아 넣으면
/// "최근 90일" 창(§12.2)이 비어 축 변화가 전부 `—` 가 되고, 월별 drift(§12.5)도
/// 뒤쪽 달에서 사라진다. 파생 층이 실제로 도는지 보려면 시간이 퍼져 있어야 한다.
fn ts_frac(frac: f64) -> Ts {
    let base = Ts::now().minus_days(210);
    Ts::from_datetime(
        base.as_datetime() + chrono::Duration::minutes((frac * 209.0 * 1440.0) as i64),
    )
}

fn axes_for(i: u64) -> Axes {
    let f = |k: u64| ((((i * k) % 17) as f64) / 16.0 * 1000.0).round() / 1000.0;
    Axes {
        chroma: f(3),
        luminance: f(5),
        density: f(7),
        grain: f(11),
        tempo: f(13),
        space: f(2),
        valence: f(23),
        intensity: f(29),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let root = args.next().unwrap_or_else(|| "/tmp/soul-demo".into());
    let n: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(120);

    let paths = Paths::at(&root);
    paths.ensure_dirs()?;
    git::ensure_repo(&paths.soul())?;
    let cfg = soul_core::config::Config::load(&paths.config_toml())?;
    cfg.save(&paths.config_toml())?;

    let store = Store::new(paths.clone());
    let db = Db::open(&paths.derived_db())?;
    let dims = cfg.embed.dims;

    let mut seq: u64 = 1;
    let mut ingest_ids: Vec<ObsId> = Vec::new();
    let embed = |db: &Db, text: &str| -> Result<Vec<f32>, Box<dyn std::error::Error>> {
        let v = fake_embed(text, dims);
        db.embed_put(
            &cache_key(rebuild::EMBED_PROVIDER, &cfg.embed.model, dims, text),
            dims,
            &v,
        )?;
        Ok(v)
    };

    // ── ingest
    for i in 0..n {
        let ts = ts_frac(i as f64 / n as f64);
        let id = id_at(ts, seq);
        let prose = format!("{} ({})", PROSE[(i % 8) as usize], i);
        let quality = match i % 6 {
            0 => Quality::Minimal,
            1 | 2 => Quality::Partial,
            _ => Quality::Full,
        };
        let kind = match i % 4 {
            0 => Kind::Text,
            1 => Kind::Image,
            2 => Kind::Audio,
            _ => Kind::Video,
        };
        let obs = Observation::Ingest(Ingest {
            id: id.clone(),
            ts,
            schema: SCHEMA_VERSION,
            source: Source {
                kind,
                sha256: format!("{:064x}", i),
                origin: format!("clipboard:{:012x}", i),
                bytes: 100 + i,
                mime: "text/plain".into(),
            },
            machine: Machine {
                prose: prose.clone(),
                axes: axes_for(i),
                tags: TAGS[(i % 4) as usize..((i % 4) + 2) as usize]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                quality,
                // 중간에 프롬프트가 한 번 바뀐 것으로 둔다 (§R11 경계가 보이도록).
                prompt_sha256: if i < n / 2 {
                    "a".repeat(64)
                } else {
                    "c".repeat(64)
                },
            },
            min_dist: if i < 4 {
                None
            } else {
                Some(((i % 9) as f64) / 20.0 + 0.2)
            },
            surprisal: if i < 14 {
                1.0
            } else {
                ((i % 10) as f64) / 10.0
            },
            model: ModelRef {
                provider: "openai".into(),
                id: "seed".into(),
                prompt_sha256: None,
                calls: vec![],
            },
            supersedes: None,
        });
        obs.validate()?;
        let v = embed(&db, &prose)?;
        db.obs_vec_put(Space::Object, id.as_str(), &v)?;
        store.append(&obs)?;
        ingest_ids.push(id);
        seq += 1;
    }

    // ── sensory reading (2/3 만 답한다 — 미완성 셀이 실제로 생기도록)
    for (i, ing) in ingest_ids.iter().enumerate() {
        if i % 3 == 2 {
            continue;
        }
        let no = i % 4 == 0;
        let prose = if no {
            Some(format!("향수 아니고 오히려 좀 서늘한 거리감 ({i})"))
        } else {
            None
        };
        let divergence = match &prose {
            Some(p) => {
                let a = embed(&db, &format!("{} ({})", PROSE[i % 8], i))?;
                let b = embed(&db, p)?;
                Some(vecmath::cosine_distance(&a, &b) as f64)
            }
            None => None,
        };
        // 대상 투입 직후에 답한 것으로 둔다.
        let ts = ts_frac(i as f64 / n as f64 + 0.001);
        let obs = Observation::Reading(Reading {
            id: id_at(ts, seq),
            ts,
            schema: SCHEMA_VERSION,
            layer: Layer::Sensory,
            target: ing.clone(),
            verdict: if no { Verdict::No } else { Verdict::Yes },
            prose,
            divergence,
        });
        obs.validate()?;
        store.append(&obs)?;
        seq += 1;
    }

    // ── context + cultural reading (절반만 — 문화 층이 빈 항목도 있어야 한다)
    for (i, ing) in ingest_ids.iter().enumerate().filter(|(i, _)| i % 2 == 0) {
        let ctx_ts = ts_frac(i as f64 / n as f64 + 0.002);
        let ctx_id = id_at(ctx_ts, seq);
        let critique = format!("{} [{}]", CRITIQUE[i % 3], i);
        let sources: Vec<SourceRef> = (0..if i % 7 == 0 { 1 } else { 3 })
            .map(|k| SourceRef {
                url: format!("https://example.invalid/{i}/{k}"),
                title: format!("근거 {k}"),
                fetched_at: ctx_ts,
            })
            .collect();
        let ctx = Observation::Context(ContextObs {
            id: ctx_id.clone(),
            ts: ctx_ts,
            schema: SCHEMA_VERSION,
            target: ing.clone(),
            critique: critique.clone(),
            lineage: vec!["슈게이즈".into(), "드림 팝".into()],
            queries: vec![format!("검색어 {i}")],
            grounded: sources.len() >= 2,
            sources,
            model: ModelRef {
                provider: "openai".into(),
                id: "seed".into(),
                prompt_sha256: Some("b".repeat(64)),
                calls: vec![],
            },
        });
        ctx.validate()?;
        let cv = embed(&db, &critique)?;
        db.obs_vec_put(Space::Critique, ctx_id.as_str(), &cv)?;
        store.append(&ctx)?;
        seq += 1;

        if i % 4 == 1 {
            continue; // 문화 카드 미응답 — 대기 목록이 비지 않도록
        }
        let no = i % 3 == 0;
        let prose = if no {
            Some(format!("그것 때문은 아니고 그냥 색이 좋았다 ({i})"))
        } else {
            None
        };
        let divergence = match &prose {
            Some(p) => {
                let b = embed(&db, p)?;
                Some(vecmath::cosine_distance(&cv, &b) as f64)
            }
            None => None,
        };
        let cr_ts = ts_frac(i as f64 / n as f64 + 0.003);
        let r = Observation::Reading(Reading {
            id: id_at(cr_ts, seq),
            ts: cr_ts,
            schema: SCHEMA_VERSION,
            layer: Layer::Cultural,
            target: ctx_id,
            verdict: if no { Verdict::No } else { Verdict::Yes },
            prose,
            divergence,
        });
        r.validate()?;
        store.append(&r)?;
        seq += 1;
    }

    git::commit_all(&paths.soul(), &format!("seed {}", store.count()?))?;
    println!(
        "관측 {}건을 {} 에 썼습니다.",
        store.count()?,
        paths.soul().display()
    );
    println!("다음: SOUL_ROOT={root} cargo run -p soul-cli -- render");
    Ok(())
}
