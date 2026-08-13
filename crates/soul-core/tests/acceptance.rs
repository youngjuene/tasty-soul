//! 인수 테스트 (§17) — `soul-core` 범위.
//!
//! **완전 오프라인이다.** 임베딩은 `common::fake_embed`로 결정론적으로 만든다.
//! 미디어·API·MCP·CLI 범위의 T번호는 각 크레이트의 `tests/`에 있다.
//!
//! T1·T12·T14가 재현성의 핵심이다. T17·T18은 각각 데이터 오염과 크래시를 막는다.

mod common;

use common::*;
use soul_core::db::embed_cache::{cache_key, Space};
use soul_core::db::Db;
use soul_core::derived::{self, cluster, Cell};
use soul_core::obs::*;
use soul_core::time::Ts;
use std::collections::BTreeMap;

const DIMS: usize = 256;

/// 픽스처를 저장소에 쓰고, 임베딩 캐시를 워밍한 상태의 샌드박스를 만든다.
fn warmed(tag: &str) -> (Sandbox, Vec<Observation>) {
    let sb = Sandbox::new(tag);
    let obs = fixture_100();
    write_fixture(&sb.paths, &obs);

    let db = Db::open(&sb.paths.derived_db()).expect("db");
    for o in &obs {
        match o {
            Observation::Ingest(i) => {
                let v = fake_embed(&i.machine.prose, DIMS);
                db.embed_put(
                    &cache_key("openai", "text-embedding-3-small", DIMS, &i.machine.prose),
                    DIMS,
                    &v,
                )
                .unwrap();
                db.obs_vec_put(Space::Object, i.id.as_str(), &v).unwrap();
            }
            Observation::Context(c) => {
                let v = fake_embed(&c.critique, DIMS);
                db.embed_put(
                    &cache_key("openai", "text-embedding-3-small", DIMS, &c.critique),
                    DIMS,
                    &v,
                )
                .unwrap();
                db.obs_vec_put(Space::Critique, c.id.as_str(), &v).unwrap();
            }
            Observation::Reading(r) => {
                // §12.7 — reading.prose 는 **텍스트 키 캐시에만** 넣는다.
                // id 색인(obs_vec/critique_vec)에 넣으면 군집과 soul_similar 가
                // 사용자 정정문을 투입 대상처럼 다루게 된다 (T49).
                if let Some(p) = &r.prose {
                    let v = fake_embed(p, DIMS);
                    db.embed_put(
                        &cache_key("openai", "text-embedding-3-small", DIMS, p),
                        DIMS,
                        &v,
                    )
                    .unwrap();
                }
            }
            _ => {}
        }
    }
    (sb, obs)
}

/// 픽스처의 모든 텍스트를 담은 조회기. `derived::compute`에 넘긴다.
fn lookup(obs: &[Observation]) -> BTreeMap<String, Vec<f32>> {
    embed_map(&fixture_texts(obs), DIMS)
}

fn compute(obs: &[Observation]) -> derived::Derived {
    let set = ObsSet::new(obs.to_vec());
    derived::compute(&set, &lookup(obs), 500)
}

// ─────────────────────────────────────────────────────── T1 · T2 · T2b

#[test]
fn t1_rebuild_from_scratch_offline_is_byte_identical() {
    let (sb, _obs) = warmed("t1");
    let (d1, missing) = soul_core::rebuild::replay(&sb.paths, true).expect("replay");
    assert!(
        missing.is_empty(),
        "캐시가 워밍되었으므로 미스가 없어야 한다: {missing:?}"
    );
    soul_core::rebuild::render_soul_md(&sb.paths, &d1, "rebuild 1").expect("render");
    let first = std::fs::read(sb.paths.soul_md()).expect("SOUL.md");

    // derived 를 통째로 날린 뒤(임베딩 캐시는 보존) 다시 만든다.
    let db = Db::open(&sb.paths.derived_db()).unwrap();
    db.clear_derived().expect("clear_derived");
    drop(db);

    let (d2, missing2) = soul_core::rebuild::replay(&sb.paths, true).expect("replay 2");
    assert!(
        missing2.is_empty(),
        "T2 — clear_derived 는 임베딩 캐시를 보존해야 한다: {missing2:?}"
    );
    soul_core::rebuild::render_soul_md(&sb.paths, &d2, "rebuild 2").expect("render 2");
    let second = std::fs::read(sb.paths.soul_md()).expect("SOUL.md");

    assert_eq!(d1, d2, "T2 — 파생값이 동일해야 한다");
    assert_eq!(
        String::from_utf8_lossy(&first),
        String::from_utf8_lossy(&second),
        "T1 — SOUL.md 가 바이트 동일해야 한다"
    );
}

#[test]
fn t2b_wiping_embed_cache_keeps_axes_within_tolerance() {
    let (sb, obs) = warmed("t2b");
    let (d1, _) = soul_core::rebuild::replay(&sb.paths, true).unwrap();

    // 임베딩까지 전부 날린다 → 오프라인 재빌드는 실패해야 한다 (§R3).
    std::fs::remove_file(sb.paths.derived_db()).unwrap();
    let offline = soul_core::rebuild::replay(&sb.paths, true);
    assert!(
        offline.is_err(),
        "T28 — 임베딩 캐시가 비면 --offline 은 에러 종료해야 한다"
    );

    // 재임베딩(여기서는 같은 결정론적 임베더)한 뒤에는 축이 허용오차 안에 들어온다.
    let (_sb2, obs2) = {
        let sb2 = Sandbox::new("t2b-warm");
        write_fixture(&sb2.paths, &obs);
        let o = obs.clone();
        (sb2, o)
    };
    let d2 = compute(&obs2);
    let a = d1.axes_final.expect("축");
    let b = d2.axes_final.expect("축");
    for ax in Axis::ALL {
        assert!(
            (a.get(ax) - b.get(ax)).abs() <= 1e-4,
            "T2b — {ax} 축이 허용오차 1e-4 를 넘었다: {} vs {}",
            a.get(ax),
            b.get(ax)
        );
    }
}

// ────────────────────────────────────────────────────────── T12 · T14 · T69

#[test]
fn t12_kmeans_is_stable_across_runs() {
    let obs = fixture_100();
    let set = ObsSet::new(obs.clone());
    let vecs: Vec<Vec<f32>> = set
        .active_ingests()
        .iter()
        .map(|i| fake_embed(&i.machine.prose, DIMS))
        .collect();

    let a = cluster::cluster(&vecs).expect("군집");
    let b = cluster::cluster(&vecs).expect("군집");
    assert_eq!(a.assignment, b.assignment, "T12 — 배정이 동일해야 한다");
    assert_eq!(a.k, b.k);
    assert_eq!(a.centroids, b.centroids);
}

#[test]
fn t14_derived_does_not_depend_on_wall_clock() {
    let obs = fixture_100();
    let a = compute(&obs);
    // 벽시계가 흐른 것과 같은 상황을 만든다 — 입력이 같으면 결과도 같아야 한다.
    std::thread::sleep(std::time::Duration::from_millis(5));
    let b = compute(&obs);
    assert_eq!(a, b, "T14 — 파생값이 실행 시각과 무관해야 한다");
    assert_eq!(
        a.t_ref,
        obs.iter().map(|o| o.ts()).max(),
        "§R1 — T_ref 는 최대 ts 다"
    );
}

#[test]
fn t69_pca_is_deterministic() {
    let vecs: Vec<Vec<f32>> = (0..40)
        .map(|i| fake_embed(&format!("항목 {i}"), DIMS))
        .collect();
    let a = derived::pca::project2(&vecs);
    let b = derived::pca::project2(&vecs);
    assert_eq!(a.len(), vecs.len());
    assert_eq!(a, b, "T69 — PCA 좌표가 동일해야 한다");
}

// ─────────────────────────────────────────────────────────────── T17 · T15

#[test]
fn t17_superseded_ingests_are_excluded_everywhere() {
    let obs = fixture_100();
    let set = ObsSet::new(obs.clone());

    let dead = set.superseded_ids();
    assert!(!dead.is_empty(), "픽스처에 supersede 사례가 있어야 한다");
    assert_eq!(
        set.active_ingests().len() + dead.len(),
        set.ingests().len(),
        "활성 + supersede = 전체"
    );
    for i in set.active_ingests() {
        assert!(
            !dead.contains(&i.id),
            "T17 — supersede된 것이 활성에 남아 있다"
        );
    }

    // supersede된 항목을 빼기 전/후로 축이 실제로 달라져야 한다 —
    // 그래야 이 필터가 동작한다는 증거가 된다.
    let without: Vec<Observation> = obs
        .iter()
        .filter(|o| {
            o.as_ingest()
                .map(|i| i.supersedes.is_none())
                .unwrap_or(true)
        })
        .cloned()
        .collect();
    let d_all = compute(&obs);
    let d_no_recast = compute(&without);
    // 활성 개수는 같다(하나가 들어오고 하나가 빠지므로). 그러나 재분석본의 축 값이
    // 원본과 다르므로 **평균은 달라야 한다** — 그것이 필터가 실제로 도는 증거다.
    assert_eq!(d_all.observation_count, d_no_recast.observation_count);
    assert_ne!(
        d_all.axes_computed, d_no_recast.axes_computed,
        "T17 — 재분석본이 원본을 대체해 축 평균이 달라져야 한다"
    );
    // 그리고 supersede된 원본의 서술은 어느 활성 집합에도 없어야 한다.
    let superseded_prose: Vec<&str> = set
        .ingests()
        .iter()
        .filter(|i| dead.contains(&i.id))
        .map(|i| i.machine.prose.as_str())
        .collect();
    for p in superseded_prose {
        assert!(
            !set.active_ingests().iter().any(|i| i.machine.prose == p),
            "T17 — supersede된 서술이 활성에 남아 있다: {p}"
        );
    }
}

#[test]
fn t15_final_axes_are_clamped_to_unit_range() {
    // 큰 offset 을 만들어 클램프를 강제한다.
    let mut obs = fixture_100();
    let mut axis_delta: AxisDelta = BTreeMap::new();
    for a in Axis::ALL {
        axis_delta.insert(a.name().into(), 0.15);
    }
    // §11.2 가드레일 한도(0.15) 안의 델타를 여러 번 누적시킨다.
    for k in 0..8u64 {
        obs.push(Observation::SoulDelta(SoulDelta {
            id: det_id(9000 + k),
            ts: ts_at(300 + k as i64, 9000 + k),
            schema: soul_core::SCHEMA_VERSION,
            window: Window {
                from: det_id(1),
                to: det_id(2),
            },
            blocks: BTreeMap::new(),
            axis_delta: axis_delta.clone(),
            morphology_delta: None,
            cites: vec![det_id(1), det_id(2), det_id(3)],
            rationale: "누적".into(),
            model: ModelRef {
                provider: "t".into(),
                id: "t".into(),
                prompt_sha256: None,
                calls: vec![],
            },
        }));
    }
    let d = compute(&obs);
    let f = d.axes_final.expect("축");
    for a in Axis::ALL {
        let v = f.get(a);
        assert!((0.0..=1.0).contains(&v), "T15 — {a} 가 [0,1] 밖이다: {v}");
    }
    assert!(
        d.axes_offset.chroma > 1.0,
        "offset 자체는 클램프되지 않는다 (final 만 클램프)"
    );
}

// ─────────────────────────────────────────────────────────── T55 · T56 · T57

#[test]
fn t55_cell_needs_both_layers() {
    let sb = Sandbox::new("t55");
    let _ = &sb;
    let mut obs = Vec::new();
    let ing = det_id(1);
    obs.push(make_ingest(&IngestSpec {
        seq: 1,
        ..Default::default()
    }));

    // 감각만 있는 상태 → 셀 없음
    obs.push(make_reading(2, 0, Layer::Sensory, &ing, Verdict::Yes, None));
    let set = ObsSet::new(obs.clone());
    assert_eq!(
        derived::divergence::cell_of(&set, &ing),
        None,
        "T55 — 한 층만 있으면 셀은 null 이다"
    );

    // context + cultural reading 이 붙으면 셀이 성립한다 (2단 조인, T55c)
    let ctx = det_id(3);
    obs.push(make_context(3, 0, &ing, 3));
    obs.push(make_reading(
        4,
        0,
        Layer::Cultural,
        &ctx,
        Verdict::No,
        Some("그것 때문은 아니다"),
    ));
    let set = ObsSet::new(obs.clone());
    assert_eq!(
        derived::divergence::cell_of(&set, &ing),
        Some(Cell::OtherReason),
        "T55c — cultural reading 이 context 를 target 으로 해도 셀이 성립해야 한다"
    );
}

#[test]
fn t55b_only_latest_context_counts_for_cell() {
    let ing = det_id(1);
    let mut obs = vec![make_ingest(&IngestSpec {
        seq: 1,
        ..Default::default()
    })];
    obs.push(make_reading(2, 0, Layer::Sensory, &ing, Verdict::Yes, None));

    // 오래된 context + 그것에 달린 응답 (yes)
    let old_ctx = det_id(3);
    obs.push(make_context(3, 0, &ing, 3));
    obs.push(make_reading(
        4,
        0,
        Layer::Cultural,
        &old_ctx,
        Verdict::Yes,
        None,
    ));
    // 최신 context + 그것에 달린 응답 (no + prose)
    let new_ctx = det_id(5);
    obs.push(make_context(5, 1, &ing, 3));
    obs.push(make_reading(
        6,
        1,
        Layer::Cultural,
        &new_ctx,
        Verdict::No,
        Some("색이 좋았을 뿐"),
    ));

    let set = ObsSet::new(obs);
    assert_eq!(
        derived::divergence::cell_of(&set, &ing),
        Some(Cell::OtherReason),
        "T55b — 셀은 최신 context 의 응답만 본다"
    );
    // 다만 이전 응답도 divergence 집계에는 남아 있다.
    let cultural: Vec<_> = set
        .readings()
        .into_iter()
        .filter(|r| r.layer == Layer::Cultural)
        .collect();
    assert_eq!(cultural.len(), 2, "T55b — 이전 응답도 관측으로 남는다");
}

#[test]
fn t56_verdict_enum_rejects_third_value() {
    let json = r#"{"id":"01J8XQZK3M7P4RSTVWXYZ0","ts":"2026-08-13T09:12:33.123Z","schema":1,
        "type":"reading","layer":"sensory","target":"01J8XQZK3M7P4RSTVWXYZ0",
        "verdict":"partial","prose":null,"divergence":null}"#;
    assert!(
        Observation::from_json(json).is_err(),
        "T56 — yes/no 외의 verdict 는 저장되지 않는다"
    );
}

#[test]
fn t57_quality_weights_are_applied() {
    assert_eq!(Quality::Full.weight(), 1.0);
    assert_eq!(Quality::Partial.weight(), 0.6);
    assert_eq!(Quality::Minimal.weight(), 0.2);

    // 같은 시각의 두 관측, 축 값이 다르고 quality 만 다르면 가중 평균이 그쪽으로 기울어야 한다.
    let t = Ts::parse("2026-01-01T00:00:00.000Z").unwrap();
    let hi = make_ingest(&IngestSpec {
        seq: 1,
        axis_base: 1.0,
        quality: Quality::Full,
        ..Default::default()
    });
    let lo = make_ingest(&IngestSpec {
        seq: 2,
        axis_base: 0.0,
        quality: Quality::Minimal,
        ..Default::default()
    });
    let set = ObsSet::new(vec![hi, lo]);
    let active = set.active_ingests();
    let m = derived::axes::weighted_mean(&active, t, false);
    // full 1.0 vs minimal 0.2 → (1.0*1.0 + 0.0*0.2) / 1.2 = 0.8333
    assert!(
        (m.chroma - 1.0 / 1.2).abs() < 1e-9,
        "T57 — quality 가중치가 반영되어야 한다: {}",
        m.chroma
    );
}

// ────────────────────────────────────────────────────── T18 · T19 · T22 · T41

#[test]
fn t18_surprisal_does_not_divide_by_zero() {
    let v = fake_embed("새 항목", DIMS);
    // |D| = 0, 군집도 없음 → 크래시 없이 1.0
    let s = derived::surprisal::compute(&v, None, &[]);
    assert_eq!(s.min_dist, None);
    assert_eq!(s.surprisal, 1.0, "T18 — |D|=0 이면 1.0 이다");

    // 군집은 있지만 |D| < 10
    let vecs: Vec<Vec<f32>> = (0..12)
        .map(|i| fake_embed(&format!("x{i}"), DIMS))
        .collect();
    let c = cluster::cluster(&vecs).expect("군집");
    let s2 = derived::surprisal::compute(&v, Some(&c), &[0.1, 0.2, 0.3]);
    assert!(s2.min_dist.is_some());
    assert_eq!(s2.surprisal, 1.0, "T18 — |D|<10 이면 1.0 이다");
}

#[test]
fn t19_coherence_is_null_with_too_few_corrections() {
    let ing = det_id(1);
    let obs = vec![
        make_ingest(&IngestSpec {
            seq: 1,
            ..Default::default()
        }),
        make_reading(2, 0, Layer::Sensory, &ing, Verdict::No, Some("한 건뿐")),
    ];
    let set = ObsSet::new(obs.clone());
    let d = derived::compute(&set, &lookup(&obs), 500);
    assert!(
        d.coherence_sensory.is_none(),
        "T19 — |R| < 2 이면 coherence 는 null 이다"
    );
    // §R10 — null 은 SOUL.md 에 — 로 렌더된다.
    let rendered = soul_core::soulmd::render(&soul_core::soulmd::RenderInput {
        derived: &d,
        profile_text: "",
        profile_rev: 0,
        human_text: "",
        divergence_examples: &[],
    });
    assert!(
        rendered.contains("일관성 —"),
        "T19 — null 이 em dash 로 렌더되어야 한다:\n{rendered}"
    );
    // 그러나 **정정 건수는 개수이므로 사라지면 안 된다.** coherence 가 null 이라고
    // 정정을 0건으로 적으면 "정정한 적이 없다"는 거짓말이 된다 (§R10은 비율에만 적용된다).
    assert_eq!(d.corrections_total, 1);
    assert!(
        rendered.contains("정정 1건"),
        "일관성이 null 이어도 정정 건수는 실제 값이어야 한다:\n{rendered}"
    );
}

#[test]
fn t22_state_at_is_stable_and_past_months_are_frozen() {
    let obs = fixture_100();
    let set = ObsSet::new(obs.clone());
    let e = lookup(&obs);
    let months = derived::state::months(&set);
    assert!(months.len() > 2, "픽스처가 여러 달에 걸쳐야 한다");

    let past = months[1];
    let a = derived::state::state_at(&set, past, &e, 500);
    let b = derived::state::state_at(&set, past, &e, 500);
    assert_eq!(a, b, "T22 — 같은 달을 두 번 계산하면 같아야 한다");

    // 현재 달에 관측 1건을 추가해도 지난 달의 상태는 변하지 않는다.
    let mut obs2 = obs.clone();
    let t_ref = set.t_ref().unwrap();
    obs2.push(make_ingest(&IngestSpec {
        seq: 8000,
        day: t_ref.days_since(Ts::parse("2026-01-01T00:00:00.000Z").unwrap()) as i64,
        ..Default::default()
    }));
    let set2 = ObsSet::new(obs2.clone());
    let c = derived::state::state_at(&set2, past, &lookup(&obs2), 500);
    assert_eq!(
        a, c,
        "T22/T40 — 관측이 추가돼도 지난 달의 state_at 은 움직이지 않는다"
    );
}

#[test]
fn t41_silhouette_sampling_is_stride_based() {
    let items: Vec<usize> = (0..1234).collect();
    let a = cluster::stride_sample(&items, 500);
    let b = cluster::stride_sample(&items, 500);
    assert_eq!(a, b, "T41 — 표본이 동일해야 한다");
    assert!(a.len() <= 500);
    // stride = ceil(1234/500) = 3
    assert_eq!(&a[..3], &[0, 3, 6]);
}

// ──────────────────────────────────────────────────────────── T45 · T49 · T28

#[test]
fn t45_vectors_are_512_bytes_at_256_dims() {
    let v = fake_embed("아무 텍스트", 256);
    assert_eq!(soul_core::vecmath::to_f16_blob(&v).len(), 512, "T45");
}

#[test]
fn t49_critique_embeddings_never_enter_the_object_space() {
    let (sb, obs) = warmed("t49");
    let db = Db::open(&sb.paths.derived_db()).unwrap();

    let object = db.obs_vec_all(Space::Object).unwrap();
    let critique = db.obs_vec_all(Space::Critique).unwrap();
    assert!(!object.is_empty() && !critique.is_empty());

    let set = ObsSet::new(obs.clone());
    let ingest_ids: std::collections::HashSet<String> =
        set.ingests().iter().map(|i| i.id.to_string()).collect();
    for (id, _) in &object {
        assert!(
            ingest_ids.contains(id),
            "T49 — Object 공간에는 ingest 만 있어야 한다: {id}"
        );
    }
    let object_ids: std::collections::HashSet<&String> = object.iter().map(|(i, _)| i).collect();
    for (id, _) in &critique {
        assert!(
            !object_ids.contains(id),
            "T49 — 두 공간이 겹치면 안 된다: {id}"
        );
    }
}

#[test]
fn t28_changing_dims_misses_the_cache() {
    let a = cache_key("openai", "text-embedding-3-small", 256, "같은 문장");
    let b = cache_key("openai", "text-embedding-3-small", 512, "같은 문장");
    let c = cache_key("openai", "text-embedding-3-large", 256, "같은 문장");
    let d = cache_key("azure", "text-embedding-3-small", 256, "같은 문장");
    assert_ne!(a, b, "T28 — dims 가 바뀌면 캐시 미스여야 한다");
    assert_ne!(a, c, "모델이 바뀌면 캐시 미스");
    assert_ne!(a, d, "제공자가 바뀌면 캐시 미스");
    assert_eq!(
        a,
        cache_key("openai", "text-embedding-3-small", 256, "같은 문장")
    );
}

// ───────────────────────────────────────────────────────────── T21b · T23 · T4b

#[test]
fn t21b_ingest_does_not_rerender_soul_md() {
    let (sb, _obs) = warmed("t21b");
    let (d, _) = soul_core::rebuild::replay(&sb.paths, true).unwrap();
    soul_core::rebuild::render_soul_md(&sb.paths, &d, "render 기준").unwrap();
    let before = std::fs::read(sb.paths.soul_md()).unwrap();
    let commits_before = soul_core::git::log_messages(&sb.paths.soul(), 1000)
        .unwrap()
        .len();

    // 관측 1건 추가 — 저장만 하고 커밋 1개. 재렌더는 없다 (§R8).
    let store = Store::new(sb.paths.clone());
    #[allow(unused)]
    let extra = make_ingest(&IngestSpec {
        seq: 7000,
        day: 400,
        ..Default::default()
    });
    let path = store.append(&extra).unwrap();
    let rel = path.strip_prefix(sb.paths.soul()).unwrap();
    soul_core::git::commit_paths(
        &sb.paths.soul(),
        &[rel],
        &format!("{} {}", extra.type_name(), extra.id()),
    )
    .unwrap();

    let after = std::fs::read(sb.paths.soul_md()).unwrap();
    assert_eq!(
        before, after,
        "T21b — ingest 1건으로 SOUL.md 가 변하면 안 된다"
    );
    let commits_after = soul_core::git::log_messages(&sb.paths.soul(), 1000).unwrap();
    assert_eq!(
        commits_after.len(),
        commits_before + 1,
        "T21 — 관측 1건 = 커밋 1개"
    );
    assert!(
        commits_after[0].starts_with("ingest 01"),
        "T21 — 메시지 형식은 `<type> <ULID>` 다: {}",
        commits_after[0]
    );
}

#[test]
fn t23_prompt_boundaries_are_reported() {
    let obs = fixture_100();
    let d = compute(&obs);
    assert!(
        d.prompt_boundaries.len() >= 2,
        "T23 — 픽스처에 프롬프트 경계가 두 개 이상 있어야 한다: {:?}",
        d.prompt_boundaries
    );
    // 경계는 **같은 종류 안에서** 값이 바뀌는 지점에만 있다. 종류가 다르면 서로 다른
    // 프롬프트 파일의 해시라 애초에 비교 대상이 아니다 (§R11).
    for kind in ["ingest", "soul_delta"] {
        let shas: Vec<&str> = d
            .prompt_boundaries
            .iter()
            .filter(|b| b.kind == kind)
            .map(|b| b.sha256.as_str())
            .collect();
        assert!(
            shas.windows(2).all(|w| w[0] != w[1]),
            "{kind} 경계는 값이 바뀌는 지점에만 있어야 한다: {shas:?}"
        );
    }
    // ULID 오름차순이어야 대시보드가 시간축에 구분선을 그을 수 있다.
    assert!(
        d.prompt_boundaries.windows(2).all(|w| w[0].id < w[1].id),
        "{:?}",
        d.prompt_boundaries
    );
}

#[test]
fn t4b_rebuild_without_soul_md_warns_and_renders_empty_human() {
    let (sb, _obs) = warmed("t4b");
    // 최초 실행이 만든 SOUL.md 를 지운다.
    let _ = std::fs::remove_file(sb.paths.soul_md());

    let (d, _) = soul_core::rebuild::replay(&sb.paths, true).unwrap();
    let report = soul_core::rebuild::render_soul_md(&sb.paths, &d, "rebuild").unwrap();
    assert!(
        !report.warnings.is_empty(),
        "T4b — SOUL.md 가 없으면 경고를 내야 한다"
    );
    let text = std::fs::read_to_string(sb.paths.soul_md()).unwrap();
    let doc = soul_core::soulmd::parse(&text).unwrap();
    assert_eq!(
        doc.human_body().map(|s| s.trim()),
        Some(""),
        "T4b — soul:human 이 빈 채로 렌더된다"
    );
}

// ─────────────────────────────────────────────────────────────────── T10 · T20

#[test]
fn t10_unbalanced_markers_fail_to_parse() {
    let bad = "# SOUL\n\n<!-- soul:gen id=header -->\n내용\n\n## 축\n";
    assert!(
        soul_core::soulmd::parse(bad).is_err(),
        "T10 — 닫히지 않은 마커는 파싱 실패다"
    );
    let bad2 = "# SOUL\n\n<!-- /soul:gen -->\n";
    assert!(
        soul_core::soulmd::parse(bad2).is_err(),
        "T10 — 짝 없는 닫는 마커도 실패다"
    );
}

#[test]
fn t20_lock_and_next_md_are_ignored_by_git() {
    let (sb, _obs) = warmed("t20");
    // 픽스처 관측을 먼저 커밋한다 — 그러지 않으면 untracked 관측 파일 때문에
    // status 가 더러워져 이 테스트가 무엇을 재는지 알 수 없게 된다.
    soul_core::git::commit_all(&sb.paths.soul(), "fixture").unwrap();
    std::fs::write(sb.paths.write_lock(), "12345").unwrap();
    std::fs::write(sb.paths.soul_next_md(), "제안").unwrap();
    assert!(
        soul_core::git::is_clean(&sb.paths.soul()).unwrap(),
        "T20 — .write.lock 과 SOUL.next.md 는 git status 에 나타나지 않는다"
    );
}

// ──────────────────────────────────────────────────────────────────── §R6 · §R10

#[test]
fn r6_observation_files_are_canonical_json() {
    let (sb, obs) = warmed("r6");
    let _store = Store::new(sb.paths.clone());
    let id = obs[0].id().clone();
    let ts = obs[0].ts();
    let raw = std::fs::read_to_string(sb.paths.observation_file(ts, &id)).unwrap();

    assert!(!raw.contains('\r'), "§R6 — CRLF 금지");
    assert!(!raw.starts_with('\u{feff}'), "§R6 — BOM 금지");
    assert!(raw.ends_with("}\n"), "파일은 개행으로 끝난다");
    // 최상위 키가 사전순인지
    let keys: Vec<&str> = raw
        .lines()
        .filter(|l| l.starts_with("  \""))
        .filter_map(|l| l.trim_start().split('"').nth(1))
        .collect();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    assert_eq!(keys, sorted, "§R6 — 키가 사전순이어야 한다: {keys:?}");
    // 재역직렬화가 원본과 같아야 한다.
    // (픽스처의 축 값은 §R6의 6자리 반올림을 이미 거쳤으므로 왕복이 성립한다.
    //  반올림되지 않은 값을 넣으면 여기서 갈리는 것이 정상이다 — 파일이 진실이다.)
    assert_eq!(&Observation::from_json(&raw).unwrap(), &obs[0]);
}

#[test]
fn r10_nulls_render_as_em_dash() {
    let d = derived::Derived::default();
    let out = soul_core::soulmd::render(&soul_core::soulmd::RenderInput {
        derived: &d,
        profile_text: "",
        profile_rev: 0,
        human_text: "",
        divergence_examples: &[],
    });
    // 8축 전부 + 헤더의 두 값이 — 여야 한다.
    for a in Axis::ALL {
        assert!(
            out.contains(&format!("| {} | — | — |", a.name())),
            "§R10 — {a} 행이 em dash 여야 한다:\n{out}"
        );
    }
    assert!(out.contains("어긋남 —"), "§R10");
    assert!(out.contains("해상도 —"), "§R10");
    assert!(
        !out.contains("대표 사례"),
        "§8.2.1 — 사례가 없으면 줄 자체를 생략한다"
    );
}

#[test]
fn axes_table_always_has_all_eight_rows_in_spec_order() {
    let obs = fixture_100();
    let d = compute(&obs);
    let out = soul_core::soulmd::render(&soul_core::soulmd::RenderInput {
        derived: &d,
        profile_text: "인공물보다 방치된 것을 고른다.",
        profile_rev: 3,
        human_text: "자유 기록",
        divergence_examples: &[],
    });
    let rows: Vec<&str> = out
        .lines()
        .filter(|l| l.starts_with("| ") && !l.starts_with("| 축 ") && !l.starts_with("|---"))
        .collect();
    assert_eq!(rows.len(), 8, "§8.2.1 — 8축 전부를 싣는다");
    for (i, a) in Axis::ALL.iter().enumerate() {
        assert!(
            rows[i].starts_with(&format!("| {} |", a.name())),
            "§7 순서를 지켜야 한다: {}",
            rows[i]
        );
    }
    // soul:human 이 이월되었는지
    assert!(out.contains("자유 기록"), "§R2 — soul:human 은 이월된다");
}
