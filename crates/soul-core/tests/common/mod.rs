//! 인수 테스트 공용 하네스 (§17).
//!
//! 픽스처는 **결정론적으로 생성한다.** 난수를 쓰면 T1·T12·T14가 실행마다 다른 것을 검사한다.
//! 임베딩은 API를 부르지 않고 텍스트에서 해시로 만든다 — 이 크레이트는 네트워크를 모른다.

#![allow(dead_code)]

use soul_core::ids::ObsId;
use soul_core::obs::*;
use soul_core::paths::Paths;
use soul_core::time::Ts;
use soul_core::SCHEMA_VERSION;
use std::collections::BTreeMap;

/// 테스트용 임시 루트. drop 시 지워진다.
pub struct Sandbox {
    pub paths: Paths,
    _dir: std::path::PathBuf,
}

impl Sandbox {
    pub fn new(name: &str) -> Sandbox {
        let base =
            std::env::temp_dir().join(format!("tasty-soul-test-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let paths = Paths::at(&base);
        paths.ensure_dirs().expect("디렉토리 생성");
        soul_core::git::ensure_repo(&paths.soul()).expect("git init");
        Sandbox { paths, _dir: base }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self._dir);
    }
}

/// 결정론적 가짜 임베딩. 같은 텍스트 → 항상 같은 벡터.
///
/// 실제 임베딩의 성질(단위 길이, 의미 근접성)을 흉내내지는 않지만,
/// **결정론과 차원만 맞으면** §12 계산의 재현성을 검사하기에 충분하다.
pub fn fake_embed(text: &str, dims: usize) -> Vec<f32> {
    use sha2::{Digest, Sha256};
    let mut out = Vec::with_capacity(dims);
    let mut counter: u32 = 0;
    while out.len() < dims {
        let mut h = Sha256::new();
        h.update(text.as_bytes());
        h.update(counter.to_le_bytes());
        for chunk in h.finalize().chunks_exact(4) {
            if out.len() == dims {
                break;
            }
            let v = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            out.push((v as f64 / u32::MAX as f64) as f32 - 0.5);
        }
        counter += 1;
    }
    soul_core::vecmath::normalized(&out)
}

/// 텍스트 → 벡터 조회기. `derived::compute` 에 넣는다.
pub fn embed_map(texts: &[String], dims: usize) -> BTreeMap<String, Vec<f32>> {
    texts
        .iter()
        .map(|t| (t.clone(), fake_embed(t, dims)))
        .collect()
}

/// ULID를 결정론적으로 만든다. `seq`가 커질수록 사전순도 커진다.
pub fn det_id(seq: u64) -> ObsId {
    // ULID = 48bit ms + 80bit 랜덤. ms를 seq로, 나머지를 0으로 두면 순서가 보장된다.
    let ms: u128 = 1_700_000_000_000u128 + seq as u128;
    let u = ulid::Ulid::from_parts(ms as u64, seq as u128);
    ObsId::parse(&u.to_string()).expect("유효한 ULID")
}

pub fn ts_at(day_offset: i64, seq: u64) -> Ts {
    let base = Ts::parse("2026-01-01T00:00:00.000Z").unwrap();
    Ts::from_datetime(
        base.as_datetime()
            + chrono::Duration::days(day_offset)
            + chrono::Duration::seconds(seq as i64),
    )
}

pub fn axes_at(v: f64) -> Axes {
    // §R6 — 파일에는 소수 6자리로 반올림되어 쓰인다. 픽스처도 같은 값이어야
    // "쓰고 다시 읽으면 같다"가 성립한다 (0.1+0.2 같은 이진 부동소수 잔차 제거).
    let r = soul_core::canon::round6;
    Axes {
        chroma: r(v),
        luminance: r((v + 0.1).min(1.0)),
        density: r((v + 0.2).min(1.0)),
        grain: r((1.0 - v).max(0.0)),
        tempo: r(v * 0.5),
        space: r((v + 0.3).min(1.0)),
        valence: r(1.0 - v * 0.5),
        intensity: r(v * 0.8),
    }
}

pub struct IngestSpec {
    pub seq: u64,
    pub day: i64,
    pub kind: Kind,
    pub quality: Quality,
    pub axis_base: f64,
    pub prompt_sha256: String,
    pub supersedes: Option<ObsId>,
}

impl Default for IngestSpec {
    fn default() -> Self {
        IngestSpec {
            seq: 0,
            day: 0,
            kind: Kind::Text,
            quality: Quality::Full,
            axis_base: 0.5,
            prompt_sha256: "aaaa".repeat(16),
            supersedes: None,
        }
    }
}

pub fn make_ingest(spec: &IngestSpec) -> Observation {
    let id = det_id(spec.seq);
    Observation::Ingest(Ingest {
        id: id.clone(),
        ts: ts_at(spec.day, spec.seq),
        schema: SCHEMA_VERSION,
        source: Source {
            kind: spec.kind,
            sha256: format!("{:064x}", spec.seq),
            origin: format!("clipboard:{:012x}", spec.seq),
            bytes: 100 + spec.seq,
            mime: "text/plain".into(),
        },
        machine: Machine {
            prose: format!("서술 {seq} — 사람이 방금 나간 자리", seq = spec.seq),
            axes: axes_at(spec.axis_base),
            tags: vec!["실내".into(), format!("태그{}", spec.seq % 5)],
            quality: spec.quality,
            prompt_sha256: spec.prompt_sha256.clone(),
        },
        min_dist: if spec.seq < 4 {
            None
        } else {
            Some(0.3 + (spec.seq % 7) as f64 * 0.05)
        },
        surprisal: if spec.seq < 14 {
            1.0
        } else {
            ((spec.seq % 10) as f64) / 10.0
        },
        model: ModelRef {
            provider: "openai".into(),
            id: "test-model".into(),
            prompt_sha256: None,
            calls: vec![format!("call_{}", spec.seq)],
        },
        supersedes: spec.supersedes.clone(),
    })
}

pub fn make_reading(
    seq: u64,
    day: i64,
    layer: Layer,
    target: &ObsId,
    verdict: Verdict,
    prose: Option<&str>,
) -> Observation {
    Observation::Reading(Reading {
        id: det_id(seq),
        ts: ts_at(day, seq),
        schema: SCHEMA_VERSION,
        layer,
        target: target.clone(),
        verdict,
        prose: prose.map(|s| s.to_string()),
        divergence: prose.map(|_| 0.41),
    })
}

pub fn make_context(seq: u64, day: i64, target: &ObsId, sources: usize) -> Observation {
    Observation::Context(ContextObs {
        id: det_id(seq),
        ts: ts_at(day, seq),
        schema: SCHEMA_VERSION,
        target: target.clone(),
        critique: format!(
            "비평 {seq}. 잔향을 악기처럼 다루던 어법을 따르되 여백을 남긴다. \
             소리의 크기가 아니라 사라지는 속도로 공간을 만든다.",
            seq = seq
        ),
        lineage: vec!["슈게이즈".into(), "드림 팝".into()],
        queries: vec![format!("질의 {seq}")],
        sources: (0..sources)
            .map(|i| SourceRef {
                url: format!("https://example.invalid/{seq}/{i}"),
                title: format!("근거 {i}"),
                fetched_at: ts_at(day, seq),
            })
            .collect(),
        grounded: sources >= 2,
        model: ModelRef {
            provider: "openai".into(),
            id: "test-model".into(),
            prompt_sha256: Some("b".repeat(64)),
            calls: vec![],
        },
    })
}

/// 관측 100건 픽스처 (§17). 네 kind, 세 quality, 2×2 셀 전부, supersede 1건을 포함한다.
pub fn fixture_100() -> Vec<Observation> {
    let mut out = Vec::new();
    let kinds = [Kind::Text, Kind::Image, Kind::Audio, Kind::Video];
    let qualities = [
        Quality::Full,
        Quality::Full,
        Quality::Partial,
        Quality::Minimal,
    ];
    let mut seq: u64 = 1;

    let mut ingest_ids = Vec::new();
    for i in 0..30u64 {
        let id_seq = seq;
        out.push(make_ingest(&IngestSpec {
            seq: id_seq,
            day: (i as i64) * 7,
            kind: kinds[(i % 4) as usize],
            quality: qualities[(i % 4) as usize],
            axis_base: 0.2 + (i % 5) as f64 * 0.15,
            prompt_sha256: if i < 20 {
                "a".repeat(64)
            } else {
                "c".repeat(64)
            },
            supersedes: None,
        }));
        ingest_ids.push(det_id(id_seq));
        seq += 1;
    }

    // 감각 응답 — yes/no 섞어서
    for (i, ing) in ingest_ids.iter().enumerate() {
        let verdict = if i % 3 == 0 {
            Verdict::No
        } else {
            Verdict::Yes
        };
        let prose = if verdict == Verdict::No {
            Some("향수 아니고 오히려 좀 서늘한 거리감")
        } else {
            None
        };
        out.push(make_reading(
            seq,
            (i as i64) * 7,
            Layer::Sensory,
            ing,
            verdict,
            prose,
        ));
        seq += 1;
    }

    // 문화 층 — 앞 24건만 (나머지는 검색 실패로 셀이 null인 상태를 만든다)
    for (i, ing) in ingest_ids.iter().take(24).enumerate() {
        let ctx_seq = seq;
        out.push(make_context(
            ctx_seq,
            (i as i64) * 7,
            ing,
            if i % 8 == 0 { 1 } else { 3 },
        ));
        seq += 1;
        let verdict = if i % 2 == 0 {
            Verdict::Yes
        } else {
            Verdict::No
        };
        let prose = if verdict == Verdict::No {
            Some("그것 때문은 아니고 그냥 색이 좋았다")
        } else {
            None
        };
        out.push(make_reading(
            seq,
            (i as i64) * 7,
            Layer::Cultural,
            &det_id(ctx_seq),
            verdict,
            prose,
        ));
        seq += 1;
    }

    // 재분석 1건 — supersedes (§R9, T17)
    out.push(make_ingest(&IngestSpec {
        seq,
        day: 210,
        kind: Kind::Image,
        quality: Quality::Full,
        axis_base: 0.9,
        prompt_sha256: "c".repeat(64),
        supersedes: Some(ingest_ids[0].clone()),
    }));
    seq += 1;

    // profile_edit 1건
    out.push(Observation::ProfileEdit(ProfileEdit {
        id: det_id(seq),
        ts: ts_at(211, seq),
        schema: SCHEMA_VERSION,
        block: "profile".into(),
        from_hash: "000000".into(),
        to_text: "인공물보다 방치된 것을 고른다.".into(),
        author: "human".into(),
    }));
    seq += 1;

    // soul_delta 1건
    let mut axis_delta: AxisDelta = BTreeMap::new();
    axis_delta.insert("grain".into(), 0.04);
    axis_delta.insert("space".into(), -0.02);
    let mut blocks: BTreeMap<String, BlockDelta> = BTreeMap::new();
    blocks.insert(
        "profile".into(),
        BlockDelta {
            from_hash: "000000".into(),
            to_text: "인공물보다 방치된 것을 고른다. 채도가 아니라 습도로 공간을 읽는다.".into(),
        },
    );
    out.push(Observation::SoulDelta(SoulDelta {
        id: det_id(seq),
        ts: ts_at(212, seq),
        schema: SCHEMA_VERSION,
        window: Window {
            from: ingest_ids[0].clone(),
            to: det_id(seq - 1),
        },
        blocks,
        axis_delta,
        morphology_delta: None,
        cites: ingest_ids.iter().take(3).cloned().collect(),
        rationale: "습기·잔향 관련 정정이 3회 반복됨".into(),
        model: ModelRef {
            provider: "openai".into(),
            id: "test-model".into(),
            prompt_sha256: Some("d".repeat(64)),
            calls: vec![],
        },
    }));
    seq += 1;

    // reaction 몇 건
    for (i, ing) in ingest_ids.iter().take(4).enumerate() {
        out.push(Observation::Reaction(Reaction {
            id: det_id(seq),
            ts: ts_at(213 + i as i64, seq),
            schema: SCHEMA_VERSION,
            target: ing.clone(),
            action: if i % 2 == 0 {
                ReactionAction::Starred
            } else {
                ReactionAction::Revisited
            },
        }));
        seq += 1;
    }

    out
}

/// 픽스처를 저장소에 쓴다. 커밋은 하지 않는다 (테스트가 필요하면 직접 한다).
pub fn write_fixture(paths: &Paths, obs: &[Observation]) {
    let store = Store::new(paths.clone());
    for o in obs {
        o.validate()
            .unwrap_or_else(|e| panic!("픽스처가 불변식을 어김 {}: {e}", o.id()));
        store.append(o).expect("픽스처 기록");
    }
}

/// §12.7 불변식 — `obs_vec`(Object)에는 **ingest 만**, `critique_vec` 에는 **context 만** 들어간다.
///
/// `reading.prose` 는 텍스트 키 캐시(`embed_cache`)에만 둔다. divergence·coherence 는
/// 텍스트로 조회하므로 그것으로 충분하고, id 색인에 섞으면 `cluster()`·`soul_similar` 이
/// 사용자 정정문을 대상처럼 다루게 된다 (§18-3).
///
/// 픽스처의 모든 임베딩 대상 텍스트.
pub fn fixture_texts(obs: &[Observation]) -> Vec<String> {
    let mut out = Vec::new();
    for o in obs {
        match o {
            Observation::Ingest(i) => out.push(i.machine.prose.clone()),
            Observation::Context(c) => out.push(c.critique.clone()),
            Observation::Reading(r) => {
                if let Some(p) = &r.prose {
                    out.push(p.clone());
                }
            }
            _ => {}
        }
    }
    out.sort();
    out.dedup();
    out
}
