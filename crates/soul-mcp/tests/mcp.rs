//! `soul-mcp` 인수 테스트 — T30·T31·T32·T34·T35·T36·T37 (§19).
//!
//! 서버 루프는 `server::handle(ctx, request)`로 분리되어 있으므로 stdin/stdout 없이
//! 프로토콜 전체를 부를 수 있다. **네트워크를 쓰는 테스트는 하나도 없다** (§19.4).

use serde_json::{json, Value};
use soul_core::db::embed_cache::Space;
use soul_core::db::Db;
use soul_core::derived::Derived;
use soul_core::ids::ObsId;
use soul_core::obs::*;
use soul_core::paths::Paths;
use soul_core::soulmd::{render, RenderInput};
use soul_core::time::Ts;
use soul_core::SCHEMA_VERSION;
use soul_mcp::rpc::Request;
use soul_mcp::server::{self, Ctx};

// ───────────────────────────────────────────────────────────────── 픽스처

/// `soul:human` 본문. 이 문장이 `soul://md`에 살아 있어야 T30이 성립한다 (§D4).
const HUMAN: &str = "사진이 아니라 사진을 찍기 직전의 망설임이 좋다. 이건 내가 직접 쓴 문장이다.";
const PROFILE: &str = "인공물보다 방치된 것을 고른다. 채도가 아니라 습도로 공간을 읽는다.";

struct Sandbox {
    paths: Paths,
}

impl Sandbox {
    fn new(tag: &str) -> Sandbox {
        let root = std::env::temp_dir().join(format!(
            "tasty-soul-mcp-{tag}-{}-{}",
            std::process::id(),
            soul_core::ids::new_id()
        ));
        let paths = Paths::at(&root);
        paths.ensure_dirs().expect("디렉토리");
        Sandbox { paths }
    }

    fn ctx(&self) -> Ctx {
        Ctx::load(self.paths.clone()).expect("컨텍스트")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // 읽기 전용 테스트(T35)가 남긴 권한을 되돌린 뒤 지운다.
        restore_writable(&self.paths.soul());
        let _ = std::fs::remove_dir_all(&self.paths.root);
    }
}

/// `01AAAA…<tail>` — 26자 ULID. 사전순이 곧 tail의 순서다.
fn id(tail: char) -> ObsId {
    let s: String = format!("01{}{tail}", "A".repeat(23));
    ObsId::parse(&s).expect("ULID")
}

fn ts(s: &str) -> Ts {
    Ts::parse(s).expect("타임스탬프")
}

fn model() -> ModelRef {
    ModelRef {
        provider: "test".into(),
        id: "m".into(),
        prompt_sha256: None,
        calls: vec![],
    }
}

#[allow(clippy::too_many_arguments)]
fn ingest(
    tail: char,
    at: &str,
    kind: Kind,
    prose: &str,
    tags: &[&str],
    chroma: f64,
    surprisal: f64,
    supersedes: Option<ObsId>,
) -> Observation {
    let mut axes = Axes::from_array([0.4; 8]);
    axes.chroma = chroma;
    Observation::Ingest(Ingest {
        id: id(tail),
        ts: ts(at),
        schema: SCHEMA_VERSION,
        source: Source {
            kind,
            sha256: "0".into(),
            origin: "file:///x".into(),
            bytes: 1,
            mime: "image/jpeg".into(),
        },
        machine: Machine {
            prose: prose.into(),
            axes,
            tags: tags.iter().map(|t| t.to_string()).collect(),
            quality: Quality::Full,
            prompt_sha256: "p1".into(),
        },
        min_dist: Some(0.4),
        surprisal,
        model: model(),
        supersedes,
    })
}

fn reading(
    tail: char,
    at: &str,
    layer: Layer,
    target: ObsId,
    verdict: Verdict,
    prose: Option<&str>,
) -> Observation {
    Observation::Reading(Reading {
        id: id(tail),
        ts: ts(at),
        schema: SCHEMA_VERSION,
        layer,
        target,
        verdict,
        prose: prose.map(|p| p.to_string()),
        divergence: prose.map(|_| 0.7),
    })
}

/// 관측 8건 · `SOUL.md` · 임베딩 캐시가 있는 완전한 아카이브.
///
/// ```text
/// ing_a(1) ← reading sensory(6, yes)
///          ← context(5) ← reading cultural(7, no)      ⇒ cell = other_reason
/// ing_b(2)
/// ing_old(3) ← reading sensory(8, no)                  ⇒ §R9로 전부 숨는다
/// ing_new(4) supersedes ing_old
/// ```
fn archive(tag: &str) -> Sandbox {
    let sb = Sandbox::new(tag);
    let store = Store::new(sb.paths.clone());

    let obs = vec![
        ingest(
            '1',
            "2026-06-10T00:00:00.000Z",
            Kind::Image,
            "차갑고 정돈된, 사람이 방금 지워진 실내",
            &["실내", "무인"],
            0.15,
            0.83,
            None,
        ),
        ingest(
            '2',
            "2026-07-01T00:00:00.000Z",
            Kind::Text,
            "느리고 건조한 문장",
            &["문장"],
            0.85,
            0.21,
            None,
        ),
        ingest(
            '3',
            "2026-07-02T00:00:00.000Z",
            Kind::Image,
            "옛 서술",
            &["실내"],
            0.50,
            0.40,
            None,
        ),
        ingest(
            '4',
            "2026-07-03T00:00:00.000Z",
            Kind::Image,
            "다시 본 서술",
            &["실내"],
            0.55,
            0.41,
            Some(id('3')),
        ),
        Observation::Context(ContextObs {
            id: id('5'),
            ts: ts("2026-07-04T00:00:00.000Z"),
            schema: SCHEMA_VERSION,
            target: id('1'),
            critique: "이 사진은 1970년대 일본 사진의 무인 실내 계보에 놓인다.".into(),
            lineage: vec!["무인 실내".into()],
            queries: vec!["무인 실내 사진".into()],
            sources: vec![
                SourceRef {
                    url: "https://example.test/a".into(),
                    title: "A".into(),
                    fetched_at: ts("2026-07-04T00:00:00.000Z"),
                },
                SourceRef {
                    url: "https://example.test/b".into(),
                    title: "B".into(),
                    fetched_at: ts("2026-07-04T00:00:00.000Z"),
                },
            ],
            grounded: true,
            model: model(),
        }),
        reading(
            '6',
            "2026-07-05T00:00:00.000Z",
            Layer::Sensory,
            id('1'),
            Verdict::Yes,
            None,
        ),
        reading(
            '7',
            "2026-07-06T00:00:00.000Z",
            Layer::Cultural,
            id('5'),
            Verdict::No,
            Some("계보가 아니라 그날의 빛 때문이다"),
        ),
        reading(
            '8',
            "2026-07-07T00:00:00.000Z",
            Layer::Sensory,
            id('3'),
            Verdict::No,
            Some("이 서술은 빗나갔다"),
        ),
    ];
    for o in &obs {
        store.append(o).expect("관측 기록");
    }

    let md = render(&RenderInput {
        derived: &Derived::default(),
        profile_text: PROFILE,
        profile_rev: 3,
        human_text: HUMAN,
        divergence_examples: &[],
    });
    std::fs::write(sb.paths.soul_md(), md.as_bytes()).expect("SOUL.md");

    // 임베딩 캐시. 쓰기 연결은 여기서 닫아 WAL을 정리한다 — 그래야 서버가
    // 읽기 전용으로 열 수 있다.
    {
        let db = Db::open(&sb.paths.derived_db()).expect("db");
        db.obs_vec_put(Space::Object, id('1').as_str(), &[1.0, 0.0, 0.0, 0.0])
            .unwrap();
        db.obs_vec_put(Space::Object, id('2').as_str(), &[0.9, 0.1, 0.0, 0.0])
            .unwrap();
        db.obs_vec_put(Space::Object, id('4').as_str(), &[0.0, 1.0, 0.0, 0.0])
            .unwrap();
        // 대체된 ingest의 벡터도 캐시에 남아 있다. 그래도 결과에 나오면 안 된다 (§R9).
        db.obs_vec_put(Space::Object, id('3').as_str(), &[1.0, 0.0, 0.0, 0.0])
            .unwrap();
    }
    sb
}

// ─────────────────────────────────────────────────────────────── 호출 헬퍼

fn request(method: &str, params: Value) -> Request {
    serde_json::from_value(json!({
        "jsonrpc": "2.0", "id": 1, "method": method, "params": params
    }))
    .expect("요청")
}

fn ok_result(ctx: &Ctx, method: &str, params: Value) -> Value {
    let r = server::handle(ctx, &request(method, params)).expect("응답이 있어야 한다");
    assert!(r.error.is_none(), "프로토콜 에러: {:?}", r.error);
    r.result.expect("result")
}

/// `(isError, 본문)`. 성공이면 본문은 파싱된 JSON, 실패면 에러 문자열이다.
fn tool(ctx: &Ctx, name: &str, args: Value) -> (bool, Value) {
    let v = ok_result(ctx, "tools/call", json!({"name": name, "arguments": args}));
    let is_error = v["isError"].as_bool().expect("isError");
    let text = v["content"][0]["text"].as_str().expect("text").to_string();
    if is_error {
        (true, Value::String(text))
    } else {
        (
            false,
            serde_json::from_str(&text).expect("툴 결과는 JSON이다"),
        )
    }
}

fn tool_ok(ctx: &Ctx, name: &str, args: Value) -> Value {
    let (is_error, v) = tool(ctx, name, args);
    assert!(!is_error, "{name} 실패: {v}");
    v
}

fn resource_text(ctx: &Ctx, uri: &str) -> String {
    let v = ok_result(ctx, "resources/read", json!({"uri": uri}));
    v["contents"][0]["text"].as_str().expect("text").to_string()
}

fn ids_of(v: &Value) -> Vec<String> {
    v.as_array()
        .expect("배열")
        .iter()
        .map(|o| o["id"].as_str().expect("id").to_string())
        .collect()
}

// ───────────────────────────────────────────────────────── T30 · T31 (§D4)

#[test]
fn t30_soul_md_resource_contains_the_human_block() {
    // §D4 — 로컬 MCP는 `soul:human`을 **포함한다.** 원격 성찰 호출과 반대다 (T29).
    let sb = archive("t30");
    let text = resource_text(&sb.ctx(), "soul://md");
    assert!(text.contains(HUMAN), "soul:human 본문이 빠졌다:\n{text}");
    assert!(
        text.contains("<!-- soul:human -->"),
        "마커 주석까지 그대로여야 한다"
    );
    assert!(text.contains("<!-- /soul:human -->"));
}

#[test]
fn t31_soul_md_resource_is_byte_identical_to_the_file() {
    // 가공하지 않는다 — 마커 주석도, 공백도, 끝 개행도 그대로다.
    let sb = archive("t31");
    let text = resource_text(&sb.ctx(), "soul://md");
    let file = std::fs::read(sb.paths.soul_md()).expect("파일");
    assert_eq!(text.as_bytes(), file.as_slice());
    // 블록 조립(§D4)을 거치면 마커가 사라진다. 그 경로를 타지 않았다는 확인.
    assert!(text.starts_with("# SOUL\n"));
    assert!(text.contains("<!-- soul:neg id=profile"));
}

// ───────────────────────────────────────────────────────────── T34 (§19.6)

#[test]
fn t34_no_write_tool_is_exposed() {
    let sb = Sandbox::new("t34");
    let tools = ok_result(&sb.ctx(), "tools/list", Value::Null);
    let names: Vec<&str> = tools["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|t| t["name"].as_str().expect("name"))
        .collect();

    // 읽기 전용 7개. 이 목록에 없는 툴은 존재하지 않는다 (§19.4).
    assert_eq!(
        names,
        vec![
            "soul_recall",
            "soul_similar",
            "soul_read",
            "soul_examples",
            "soul_readings",
            "soul_context",
            "soul_timeline",
        ]
    );

    // 이름에 쓰기 동사가 섞이는 순간 취향 모델이 자기 출력으로 오염되기 시작한다.
    const WRITE_VERBS: [&str; 14] = [
        "write", "append", "create", "update", "delete", "remove", "set", "put", "edit", "save",
        "add", "record", "commit", "propose",
    ];
    for n in &names {
        for v in WRITE_VERBS {
            assert!(!n.contains(v), "쓰기 툴로 보이는 이름: {n} ({v})");
        }
    }

    // 서버 인스트럭션도 쓰기 툴이 없다고 못박는다.
    let init = ok_result(&sb.ctx(), "initialize", json!({}));
    assert!(init["instructions"]
        .as_str()
        .unwrap()
        .contains("쓰기 툴은 없다"));
}

// ───────────────────────────────────────────────────────── T37 (§19.2.1)

#[test]
fn t37_every_description_is_one_line_under_120_chars() {
    let sb = Sandbox::new("t37");
    let tools = ok_result(&sb.ctx(), "tools/list", Value::Null);

    for t in tools["tools"].as_array().expect("tools") {
        let name = t["name"].as_str().expect("name");
        let d = t["description"]
            .as_str()
            .unwrap_or_else(|| panic!("{name}: description 없음"));
        assert!(!d.contains('\n'), "{name}: 설명문은 한 줄이다");
        assert!(
            d.chars().count() <= 120,
            "{name}: 설명문 {}자 (120자 초과)",
            d.chars().count()
        );
    }

    // 파라미터 설명도 같은 예산을 쓴다. 자명한 것은 아예 없다.
    let mut nested = 0usize;
    collect_descriptions(&tools, &mut |d| {
        nested += 1;
        assert!(
            d.chars().count() <= 120,
            "긴 설명문 ({}자): {d}",
            d.chars().count()
        );
    });
    assert!(nested >= 7, "설명문을 하나도 찾지 못했다");

    // 리소스 설명도 마찬가지다.
    let res = ok_result(&sb.ctx(), "resources/list", Value::Null);
    collect_descriptions(&res, &mut |d| {
        assert!(d.chars().count() <= 120, "리소스 설명문이 길다: {d}");
    });
}

fn collect_descriptions(v: &Value, f: &mut impl FnMut(&str)) {
    match v {
        Value::Object(m) => {
            for (k, val) in m {
                if k == "description" {
                    if let Some(s) = val.as_str() {
                        f(s);
                    }
                }
                collect_descriptions(val, f);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| collect_descriptions(x, f)),
        _ => {}
    }
}

// ─────────────────────────────────────────────────────────────────── T36

#[test]
fn t36_similar_without_derived_db_is_a_clear_error_not_a_crash() {
    // 캐시 파일이 없는 상태. 서버는 뜨고, 다른 툴은 멀쩡히 돌고, 유사도만 막힌다.
    let sb = Sandbox::new("t36");
    let store = Store::new(sb.paths.clone());
    store
        .append(&ingest(
            '1',
            "2026-06-10T00:00:00.000Z",
            Kind::Image,
            "실내",
            &[],
            0.2,
            0.5,
            None,
        ))
        .unwrap();
    let ctx = sb.ctx();
    assert!(
        ctx.db.is_none(),
        "픽스처에 derived.sqlite 가 있으면 안 된다"
    );

    let (is_error, msg) = tool(
        &ctx,
        "soul_similar",
        json!({"observation_id": id('1').as_str()}),
    );
    assert!(is_error, "에러여야 한다");
    let msg = msg.as_str().unwrap();
    assert!(
        msg.contains("derived.sqlite"),
        "무엇이 없는지 말해야 한다: {msg}"
    );
    assert!(
        msg.contains("soul rebuild"),
        "무엇을 하면 되는지 말해야 한다: {msg}"
    );

    // 나머지 경로는 캐시 없이도 동작한다 — 캐시는 삭제 가능한 것이다 (§3).
    assert_eq!(ids_of(&tool_ok(&ctx, "soul_recall", json!({}))).len(), 1);
    assert!(tool_ok(&ctx, "soul_timeline", json!({})).is_array());
}

// ─────────────────────────────────────────────────────────────────── T35

#[cfg(unix)]
#[test]
fn t35_server_works_with_a_read_only_soul_dir() {
    use std::os::unix::fs::PermissionsExt;

    let sb = archive("t35");
    set_mode(&sb.paths.soul(), 0o555, 0o444);

    let ctx = sb.ctx();
    // 리소스·툴 전부 정상이어야 한다.
    assert!(resource_text(&ctx, "soul://md").contains(HUMAN));
    assert_eq!(ids_of(&tool_ok(&ctx, "soul_recall", json!({}))).len(), 3);
    assert!(!tool_ok(
        &ctx,
        "soul_similar",
        json!({"observation_id": id('1').as_str()})
    )
    .as_array()
    .unwrap()
    .is_empty());

    // 서버가 아무것도 쓰지 않았다는 것을 권한으로 확인한다.
    let mode = std::fs::metadata(sb.paths.soul())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o555);

    restore_writable(&sb.paths.soul());
}

#[cfg(unix)]
fn set_mode(path: &std::path::Path, dir_mode: u32, file_mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    // 안쪽부터 잠근다 — 디렉토리를 먼저 잠그면 그 안으로 못 들어간다.
    if let Ok(rd) = std::fs::read_dir(path) {
        for e in rd.flatten() {
            set_mode(&e.path(), dir_mode, file_mode);
        }
    }
    let is_dir = path.is_dir();
    let _ = std::fs::set_permissions(
        path,
        std::fs::Permissions::from_mode(if is_dir { dir_mode } else { file_mode }),
    );
}

#[cfg(unix)]
fn restore_writable(path: &std::path::Path) {
    set_mode(path, 0o755, 0o644);
}

#[cfg(not(unix))]
fn restore_writable(_path: &std::path::Path) {}

// ───────────────────────────────────────────────────── T32 (구조적 보장)

#[test]
fn t32_dependencies_stay_minimal() {
    // MCP SDK 하나가 전이 의존으로 HTTP 클라이언트를 끌고 들어온다.
    // "아웃바운드 요청 0건"은 코드가 아니라 의존성 그래프로 보장한다 (§19.4).
    let toml = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
    )
    .expect("Cargo.toml");
    let deps: Vec<String> = toml
        .split("[dependencies]")
        .nth(1)
        .expect("[dependencies]")
        .lines()
        .take_while(|l| !l.trim_start().starts_with('['))
        .filter_map(|l| l.split_once('=').map(|(k, _)| k.trim().to_string()))
        .filter(|k| !k.is_empty() && !k.starts_with('#'))
        .collect();
    assert_eq!(
        deps,
        vec!["anyhow", "serde", "serde_json", "soul-core", "thiserror"],
        "soul-mcp 의 의존성이 바뀌었다"
    );
}

// ─────────────────────────────────────────────────────────── 프로토콜 표면

#[test]
fn initialize_handshake_then_notification_then_ping() {
    let sb = Sandbox::new("proto");
    let ctx = sb.ctx();

    let init = ok_result(&ctx, "initialize", json!({"protocolVersion": "2025-03-26"}));
    assert_eq!(
        init["protocolVersion"], "2025-03-26",
        "클라이언트 버전을 그대로 돌려준다"
    );
    assert_eq!(init["serverInfo"]["name"], "soul");
    assert!(init["serverInfo"]["version"].is_string());
    assert!(init["capabilities"]["resources"].is_object());
    assert!(init["capabilities"]["tools"].is_object());

    // id 없는 notification 에는 응답하지 않는다.
    let n: Request =
        serde_json::from_value(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .unwrap();
    assert!(server::handle(&ctx, &n).is_none());

    assert_eq!(ok_result(&ctx, "ping", Value::Null), json!({}));

    // 알 수 없는 메서드는 -32601.
    let r = server::handle(&ctx, &request("soul/무언가", Value::Null)).unwrap();
    assert_eq!(r.error.expect("에러").code, -32601);
}

#[test]
fn all_four_resources_read() {
    let sb = archive("res");
    let ctx = sb.ctx();

    let listed: Vec<String> = ok_result(&ctx, "resources/list", Value::Null)["resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["uri"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        listed,
        [
            "soul://md",
            "soul://axes",
            "soul://divergence",
            "soul://stats"
        ]
    );

    let axes: Value = serde_json::from_str(&resource_text(&ctx, "soul://axes")).unwrap();
    assert_eq!(axes["axes"].as_array().unwrap().len(), 8);
    assert_eq!(axes["axes"][0]["axis"], "chroma");
    assert!(
        axes["axes"][0]["value"].is_number(),
        "관측이 있으면 값이 나온다"
    );
    assert!(
        axes["axes"][0]["low"].is_string(),
        "§7 축 정의가 함께 실린다"
    );

    let div: Value = serde_json::from_str(&resource_text(&ctx, "soul://divergence")).unwrap();
    // ing_a 하나가 (yes, no) = other_reason 이다. ing_old 는 §R9로 세지 않는다.
    assert_eq!(div["cells"]["other_reason"], 1);
    assert_eq!(div["cells"]["total"], 1);
    assert!(div["legend"]["other_reason"].is_string());

    let stats: Value = serde_json::from_str(&resource_text(&ctx, "soul://stats")).unwrap();
    assert_eq!(stats["observations"], 3, "활성 ingest 만 센다 (§R9)");
    assert_eq!(stats["observations_total"], 8);
    assert_eq!(stats["period"]["first"], "2026-06-10T00:00:00.000Z");
    assert_eq!(stats["counts_by_kind"]["image"], 2);
}

// ─────────────────────────────────────────────────────────────── 툴 동작

#[test]
fn recall_filters_by_tag_kind_and_period() {
    let sb = archive("recall");
    let ctx = sb.ctx();

    // 기본은 최신순 전체(활성 ingest 3건).
    assert_eq!(
        ids_of(&tool_ok(&ctx, "soul_recall", json!({}))),
        vec![
            id('4').to_string(),
            id('2').to_string(),
            id('1').to_string()
        ]
    );
    // 태그는 AND.
    assert_eq!(
        ids_of(&tool_ok(
            &ctx,
            "soul_recall",
            json!({"tags": ["실내", "무인"]})
        )),
        vec![id('1').to_string()]
    );
    assert_eq!(
        ids_of(&tool_ok(&ctx, "soul_recall", json!({"kind": "text"}))),
        vec![id('2').to_string()]
    );
    assert_eq!(
        ids_of(&tool_ok(
            &ctx,
            "soul_recall",
            json!({"since": "2026-07-02"})
        )),
        vec![id('4').to_string()]
    );
    assert_eq!(
        ids_of(&tool_ok(
            &ctx,
            "soul_recall",
            json!({"until": "2026-06-30", "limit": 5})
        )),
        vec![id('1').to_string()]
    );

    // 요약은 §19.4의 6개 키뿐이다 — axes 와 임베딩은 없다.
    let first = &tool_ok(&ctx, "soul_recall", json!({"limit": 1}))[0];
    let mut keys: Vec<&str> = first
        .as_object()
        .unwrap()
        .keys()
        .map(|s| s.as_str())
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["id", "kind", "prose", "surprisal", "tags", "ts"]);

    // 잘못된 kind 는 크래시가 아니라 설명이 붙은 에러다.
    let (is_error, msg) = tool(&ctx, "soul_recall", json!({"kind": "gif"}));
    assert!(is_error && msg.as_str().unwrap().contains("gif"));
}

#[test]
fn recall_date_bounds_include_both_ends_of_the_day() {
    // "경계 포함"이라고 써 놓고 `until=그 날` 이 그 날 00:00 에서 끊기면
    // 그 날에 기록한 관측이 통째로 사라진다. 하루 안에서 시각이 흩어진 픽스처로 확인한다.
    let sb = Sandbox::new("bounds");
    let store = Store::new(sb.paths.clone());
    for (tail, at) in [
        ('1', "2026-07-03T00:00:00.000Z"),
        ('2', "2026-07-03T12:30:00.000Z"),
        ('3', "2026-07-03T23:59:59.999Z"),
        ('4', "2026-07-04T00:00:00.000Z"),
    ] {
        store
            .append(&ingest(tail, at, Kind::Image, "하루", &[], 0.5, 0.5, None))
            .unwrap();
    }
    let ctx = sb.ctx();

    let day = ids_of(&tool_ok(
        &ctx,
        "soul_recall",
        json!({"until": "2026-07-03"}),
    ));
    assert_eq!(
        day,
        vec![
            id('3').to_string(),
            id('2').to_string(),
            id('1').to_string()
        ],
        "그 날의 마지막 순간까지 포함해야 한다"
    );

    // 하한도 같은 날을 통째로 잡는다.
    assert_eq!(
        ids_of(&tool_ok(
            &ctx,
            "soul_recall",
            json!({"since": "2026-07-04"})
        )),
        vec![id('4').to_string()]
    );

    // 달 단위도 마찬가지다 — 7월 전체가 들어온다.
    assert_eq!(
        ids_of(&tool_ok(&ctx, "soul_recall", json!({"until": "2026-07"}))).len(),
        4
    );
    assert!(ids_of(&tool_ok(&ctx, "soul_recall", json!({"until": "2026-06"}))).is_empty());

    // RFC3339 로 특정 순간을 주면 하루로 늘리지 않고 거기서 끊는다.
    assert_eq!(
        ids_of(&tool_ok(
            &ctx,
            "soul_recall",
            json!({"until": "2026-07-03T12:30:00.000Z"})
        )),
        vec![id('2').to_string(), id('1').to_string()]
    );
}

#[test]
fn similar_uses_cached_vectors_and_hides_superseded() {
    // T33 — 임베딩 API 호출 없이 캐시 벡터만으로 순위가 나온다.
    let sb = archive("similar");
    let ctx = sb.ctx();
    let out = ids_of(&tool_ok(
        &ctx,
        "soul_similar",
        json!({"observation_id": id('1').as_str()}),
    ));

    // ing_old(3)는 ing_a와 벡터가 같지만 대체되었으므로 나오지 않는다 (§R9).
    assert_eq!(out, vec![id('2').to_string(), id('4').to_string()]);
    assert!(!out.contains(&id('1').to_string()), "자기 자신은 빠진다");

    assert_eq!(
        ids_of(&tool_ok(
            &ctx,
            "soul_similar",
            json!({"observation_id": id('1').as_str(), "n": 1})
        )),
        vec![id('2').to_string()]
    );

    // 대체된 ingest 로는 아예 물을 수 없다.
    let (is_error, msg) = tool(
        &ctx,
        "soul_similar",
        json!({"observation_id": id('3').as_str()}),
    );
    assert!(is_error);
    assert!(msg.as_str().unwrap().contains("대체"), "{msg}");
}

#[test]
fn read_returns_the_whole_observation_but_refuses_superseded() {
    let sb = archive("read");
    let ctx = sb.ctx();

    let v = tool_ok(
        &ctx,
        "soul_read",
        json!({"observation_id": id('1').as_str()}),
    );
    assert_eq!(v["type"], "ingest");
    assert_eq!(
        v["machine"]["axes"]["chroma"], 0.15,
        "전문에는 축이 그대로 있다"
    );

    let (is_error, msg) = tool(
        &ctx,
        "soul_read",
        json!({"observation_id": id('3').as_str()}),
    );
    assert!(is_error);
    let msg = msg.as_str().unwrap();
    assert!(
        msg.contains(id('4').as_str()),
        "대체본을 알려줘야 한다: {msg}"
    );

    // 잘못된 ID 형식도 크래시가 아니다.
    let (is_error, _) = tool(&ctx, "soul_read", json!({"observation_id": "01"}));
    assert!(is_error);
}

#[test]
fn examples_sorts_by_axis_pole() {
    let sb = archive("examples");
    let ctx = sb.ctx();
    // chroma: ing_a 0.15 · ing_new 0.55 · ing_b 0.85 (ing_old 0.50 은 §R9로 제외)
    assert_eq!(
        ids_of(&tool_ok(
            &ctx,
            "soul_examples",
            json!({"axis": "chroma", "pole": "high", "n": 2})
        )),
        vec![id('2').to_string(), id('4').to_string()]
    );
    assert_eq!(
        ids_of(&tool_ok(
            &ctx,
            "soul_examples",
            json!({"axis": "chroma", "pole": "low", "n": 1})
        )),
        vec![id('1').to_string()]
    );
    let (is_error, msg) = tool(
        &ctx,
        "soul_examples",
        json!({"axis": "novelty", "pole": "high"}),
    );
    assert!(is_error, "surprisal 은 축이 아니다 (§7)");
    assert!(
        msg.as_str().unwrap().contains("chroma"),
        "가능한 축을 알려준다"
    );
}

#[test]
fn readings_walk_the_two_step_join_for_the_cell_filter() {
    let sb = archive("readings");
    let ctx = sb.ctx();

    let all = tool_ok(&ctx, "soul_readings", json!({}));
    let targets: Vec<&str> = all
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["target"].as_str().unwrap())
        .collect();
    // reading(8)은 대체된 ing_old 를 가리키므로 나오지 않는다 (§R9).
    assert_eq!(targets, vec![id('5').as_str(), id('1').as_str()]);

    // cultural reading 의 target 은 context 다. 셀은 그 context 의 target ingest 로
    // 거슬러 올라가야 정해진다 (T55c).
    let cell = tool_ok(&ctx, "soul_readings", json!({"cell": "other_reason"}));
    assert_eq!(cell.as_array().unwrap().len(), 2);
    assert!(tool_ok(&ctx, "soul_readings", json!({"cell": "read"}))
        .as_array()
        .unwrap()
        .is_empty());

    let no = tool_ok(&ctx, "soul_readings", json!({"verdict": "no"}));
    assert_eq!(no.as_array().unwrap().len(), 1);
    assert_eq!(no[0]["layer"], "cultural");
    assert_eq!(no[0]["prose"], "계보가 아니라 그날의 빛 때문이다");

    // verdict=yes 면 prose 는 언제나 null 이다 (§6.3, T7).
    let yes = tool_ok(&ctx, "soul_readings", json!({"verdict": "yes"}));
    assert!(yes[0]["prose"].is_null());

    let (is_error, _) = tool(&ctx, "soul_readings", json!({"cell": "그런셀없음"}));
    assert!(is_error);
}

#[test]
fn context_returns_the_latest_one_without_the_queries() {
    let sb = archive("context");
    let ctx = sb.ctx();
    let v = tool_ok(&ctx, "soul_context", json!({"ingest_id": id('1').as_str()}));
    assert!(v["critique"].as_str().unwrap().contains("계보"));
    assert_eq!(v["lineage"][0], "무인 실내");
    assert_eq!(v["sources"].as_array().unwrap().len(), 2);
    assert_eq!(v["grounded"], true);
    // §D6 — 실제 검색어는 내보내지 않는다.
    assert!(v.get("queries").is_none());

    let (is_error, msg) = tool(&ctx, "soul_context", json!({"ingest_id": id('2').as_str()}));
    assert!(is_error, "비평이 없으면 빈 값이 아니라 사유를 준다");
    assert!(msg.as_str().unwrap().contains("context"));
}

#[test]
fn timeline_is_calendar_months() {
    let sb = archive("timeline");
    let ctx = sb.ctx();
    let all = tool_ok(&ctx, "soul_timeline", json!({}));
    let months: Vec<&str> = all
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["month"].as_str().unwrap())
        .collect();
    assert_eq!(months, vec!["2026-06", "2026-07"]);
    // 누적이므로 단조 증가한다.
    assert_eq!(all[0]["n"], 1);
    assert_eq!(all[1]["n"], 3);

    let one = tool_ok(&ctx, "soul_timeline", json!({"month": "2026-07"}));
    assert_eq!(one["month"], "2026-07");
    assert!(one.is_object(), "month 를 주면 하나만 준다");

    let (is_error, _) = tool(&ctx, "soul_timeline", json!({"month": "2020-01"}));
    assert!(is_error, "기간 밖이면 사유를 준다");
    let (is_error, _) = tool(&ctx, "soul_timeline", json!({"month": "작년"}));
    assert!(is_error);
}

#[test]
fn config_caps_both_the_count_and_the_prose_length() {
    // §19.9 — max_recall_limit · prose_max_chars 는 설정이 정한다.
    let sb = archive("caps");
    let mut cfg = soul_core::config::Config::default();
    cfg.mcp.max_recall_limit = 2;
    cfg.mcp.prose_max_chars = 5;
    cfg.save(&sb.paths.config_toml()).expect("config");

    let ctx = sb.ctx();
    let v = tool_ok(&ctx, "soul_recall", json!({"limit": 50}));
    assert_eq!(v.as_array().unwrap().len(), 2, "상한을 넘길 수 없다");
    assert_eq!(ids_of(&v), vec![id('4').to_string(), id('2').to_string()]);
    // 문자 단위로 자른다 — 한글이 바이트로 잘리면 글자가 깨진다.
    assert_eq!(v[1]["prose"], "느리고 건…", "5자 + 말줄임표");
}
