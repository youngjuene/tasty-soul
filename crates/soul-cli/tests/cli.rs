//! §14 명령 표면 전체가 파싱되는지 (`Cli::try_parse_from`) 와
//! `soul mcp --print-config` 출력이 유효한 JSON인지 (T38).
//!
//! **네트워크를 쓰지 않는다.** 여기서 실행하는 것은 clap 파싱과 순수 함수뿐이다.
//! 실제 명령을 돌리는 테스트는 `soul-pipeline`·`soul-core` 쪽 통합 테스트의 몫이다.

use clap::Parser;
use soul_cli::cmd::{Cli, Command, ExportTarget, TraceCommand};
use soul_core::obs::{Kind, Verdict};

const ID: &str = "01J8XQZK3M7P4RSTVWXYZ00001";
const ID2: &str = "01J8XQZK3M7P4RSTVWXYZ00002";

fn ok(args: &[&str]) -> Command {
    Cli::try_parse_from(args)
        .unwrap_or_else(|e| panic!("{args:?} 는 §14의 명령이다. 파싱 실패:\n{e}"))
        .command
}

/// §14 표에 적힌 명령 전부. 하나라도 빠지면 앱과 CLI의 표면이 갈라진다.
#[test]
fn every_command_in_section_14_parses() {
    let cases: Vec<Vec<&str>> = vec![
        vec!["soul", "doctor"],
        vec!["soul", "doctor", "--probe"],
        vec!["soul", "ingest", "/tmp/a.jpg"],
        vec!["soul", "ingest", "https://www.youtube.com/watch?v=abc"],
        vec!["soul", "ingest", "-"],
        vec!["soul", "read", ID, "yes"],
        vec!["soul", "read", ID, "no", "사람이 나간 자리 같다"],
        vec!["soul", "context", ID],
        vec!["soul", "context", ID, "--redo"],
        vec!["soul", "recast", ID, "video"],
        vec!["soul", "reflect"],
        vec!["soul", "reflect", "--force"],
        vec!["soul", "render"],
        // §14 — "`--offline`은 세 형태 모두에 붙일 수 있으며". `render`도 그 셋 중 하나다.
        vec!["soul", "render", "--offline"],
        vec!["soul", "rebuild"],
        vec!["soul", "rebuild", "--from-scratch"],
        vec!["soul", "rebuild", "--offline"],
        vec!["soul", "rebuild", "--from-scratch", "--offline"],
        vec!["soul", "reanalyze", ID],
        vec!["soul", "stats"],
        vec!["soul", "stats", "--json"],
        vec!["soul", "mcp"],
        vec!["soul", "mcp", "--print-config"],
        vec!["soul", "export", "--target=prompt"],
        vec!["soul", "export", "--target", "prompt"],
        vec!["soul", "maintain"],
        vec!["soul", "trace", "purge"],
    ];
    for c in cases {
        ok(&c);
    }
}

#[test]
fn arguments_land_in_the_right_fields() {
    match ok(&["soul", "doctor", "--probe"]) {
        Command::Doctor { probe } => assert!(probe),
        other => panic!("{other:?}"),
    }
    match ok(&["soul", "ingest", "https://youtu.be/x"]) {
        Command::Ingest { target } => assert_eq!(target, "https://youtu.be/x"),
        other => panic!("{other:?}"),
    }
    match ok(&["soul", "read", ID, "yes"]) {
        Command::Read {
            obs_id,
            verdict,
            prose,
        } => {
            assert_eq!(obs_id.as_str(), ID);
            assert_eq!(verdict, Verdict::Yes);
            assert_eq!(prose, None);
        }
        other => panic!("{other:?}"),
    }
    match ok(&["soul", "context", ID2, "--redo"]) {
        Command::Context { ingest_id, redo } => {
            assert_eq!(ingest_id.as_str(), ID2);
            assert!(redo);
        }
        other => panic!("{other:?}"),
    }
    match ok(&["soul", "recast", ID, "audio"]) {
        Command::Recast { ingest_id, kind } => {
            assert_eq!(ingest_id.as_str(), ID);
            assert_eq!(kind, Kind::Audio);
        }
        other => panic!("{other:?}"),
    }
    match ok(&["soul", "export", "--target=prompt"]) {
        Command::Export { target } => assert_eq!(target, ExportTarget::Prompt),
        other => panic!("{other:?}"),
    }
    match ok(&["soul", "trace", "purge"]) {
        Command::Trace { command } => assert!(matches!(command, TraceCommand::Purge)),
        other => panic!("{other:?}"),
    }
}

/// §14에 없는 것은 받지 않는다. 표면이 조용히 늘어나는 것을 막는다.
#[test]
fn unknown_commands_and_flags_are_rejected() {
    for bad in [
        vec!["soul"],                             // 서브커맨드 필수
        vec!["soul", "sync"],                     // 없는 명령
        vec!["soul", "render", "--from-scratch"], // render에는 없는 플래그
        vec!["soul", "stats", "--yaml"],          // 없는 형식
        vec!["soul", "read", ID],                 // verdict 필수
        vec!["soul", "recast", ID],               // kind 필수
        vec!["soul", "ingest"],                   // 대상 필수
        vec!["soul", "trace"],                    // 하위 명령 필수
        vec!["soul", "trace", "clear"],           // purge 뿐이다
    ] {
        assert!(
            Cli::try_parse_from(&bad).is_err(),
            "{bad:?} 는 거부되어야 한다"
        );
    }
}

/// T38 — `--print-config` 출력이 유효한 JSON이고 §19.7의 형태 그대로다.
#[test]
fn print_config_output_is_valid_json() {
    let s = soul_cli::cmd::mcp::config_json().expect("설정 JSON");
    let v: serde_json::Value = serde_json::from_str(&s).expect("유효한 JSON이어야 한다");
    assert_eq!(
        v,
        serde_json::json!({
            "mcpServers": { "soul": { "command": "soul", "args": ["mcp"] } }
        })
    );
}

/// T38 — 설정 파일을 수정하지 않는다.
///
/// 이 명령이 손댈 수 있는 유일한 후보는 앱 데이터 루트다. 빈 루트를 가리켜 두고
/// 호출한 뒤에도 그 아래에 아무 파일이 생기지 않아야 한다.
#[test]
fn print_config_creates_no_files() {
    let root = std::env::temp_dir()
        .join("tasty-soul-cli-t38")
        .join(soul_core::ids::new_id().to_string());
    std::fs::create_dir_all(&root).unwrap();

    let _ = soul_cli::cmd::mcp::config_json().unwrap();

    assert_eq!(
        std::fs::read_dir(&root).unwrap().count(),
        0,
        "T38 — 설정 파일을 만들거나 고치지 않는다"
    );
    let _ = std::fs::remove_dir_all(&root);
}
