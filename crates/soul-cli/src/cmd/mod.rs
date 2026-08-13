//! CLI 디스패치 (§14).
//!
//! ```text
//! soul doctor [--probe]
//! soul ingest <path|url|->             # - 는 stdin에서 텍스트를 읽는다
//! soul read <obs_id> <verdict> [prose]
//! soul context <ingest_id> [--redo]
//! soul recast <ingest_id> <kind>
//! soul reflect [--force]
//! soul render
//! soul rebuild [--from-scratch] [--offline]
//! soul reanalyze <obs_id>
//! soul stats [--json]
//! soul mcp [--print-config]
//! soul export --target=prompt
//! soul maintain
//! soul trace purge
//! soul secrets set|status|delete <openai|search|youtube>   # 키체인 (§2)
//! ```
//!
//! | 명령 | derived.sqlite | SOUL.md | 커밋 |
//! |---|---|---|---|
//! | `soul render` | 읽기만 | 재작성 | `render <T_ref>` |
//! | `soul rebuild` | 관측 재생으로 갱신 | 재작성 | `rebuild <n>` |
//! | `soul rebuild --from-scratch` | **삭제 후 전량 재구축** | 재작성 | `rebuild <n>` |
//!
//! `--offline`은 위 **세 형태 모두**에 붙으며 임베딩 캐시 미스 시 에러 종료한다 (§R3, T28).
//!
//! ## 이 파일의 규약
//!
//! - 출력은 한국어. 사람이 읽는 결과는 stdout, 경고·오류·진행 상황은 stderr다.
//!   `soul export`처럼 리다이렉트해 쓰는 명령이 있으므로 이 구분을 지킨다 (§19.8).
//! - 실패 시 종료 코드는 **1**이다. clap의 기본값(2)을 그대로 두지 않는다.
//! - **`#[tokio::main]`을 쓰지 않는다.** `soul mcp`는 런타임 없이 exec 해야 하고(§19.7),
//!   `render`·`rebuild --offline`·`stats`·`export`는 애초에 비동기가 아니다.
//!   런타임은 실제로 필요한 명령에서 [`block_on`]이 만든다.

pub mod export;
pub mod maintain;
pub mod mcp;
pub mod pipeline;
pub mod rebuild;
pub mod secrets;
pub mod stats;

use clap::{Parser, Subcommand, ValueEnum};
use soul_core::ids::ObsId;
use soul_core::obs::{Kind, Verdict};
use soul_core::paths::Paths;

#[derive(Debug, Parser)]
#[command(
    name = "soul",
    version,
    about = "tasty-soul — 취향 관측 로그 (§14)",
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// 키·모델 ID·ffmpeg 검증 (§9.9)
    Doctor {
        /// 슬롯마다 최소 호출을 날려 modality 지원 여부까지 확인한다 (네트워크 사용)
        #[arg(long)]
        probe: bool,
    },

    /// 투입 (§9.1). `-` 면 stdin에서 텍스트를 읽는다
    Ingest {
        #[arg(value_name = "PATH|URL|-", allow_hyphen_values = true)]
        target: String,
    },

    /// ○/× 응답 기록 (§6.3). verdict는 yes | no 뿐이다 (T56)
    Read {
        obs_id: ObsId,
        #[arg(value_parser = parse_verdict, value_name = "yes|no")]
        verdict: Verdict,
        /// `no`일 때만 의미가 있다. 있으면 divergence를 계산한다 (T6·T7)
        prose: Option<String>,
    },

    /// 문화 글귀 생성 (§9.10). 평시엔 투입마다 자동으로 돈다
    Context {
        ingest_id: ObsId,
        /// 이미 context가 있어도 다시 만든다
        #[arg(long)]
        redo: bool,
    },

    /// YouTube 항목의 kind 뒤집기 (§9.3). 새 ingest + supersedes를 만든다
    Recast {
        ingest_id: ObsId,
        #[arg(value_parser = parse_kind, value_name = "text|image|audio|video")]
        kind: Kind,
    },

    /// 성찰 (§11.2). 제안은 SOUL.next.md에 쓰고 승인은 앱에서 받는다
    Reflect {
        /// §11.2 트리거 조건을 무시하고 즉시 실행한다
        #[arg(long)]
        force: bool,
    },

    /// SOUL.md 재작성 (§R8). derived.sqlite는 **읽기만** 한다
    Render {
        /// 임베딩 캐시 미스 시 에러 종료 (§R3, T28)
        #[arg(long)]
        offline: bool,
    },

    /// 관측 재생으로 파생값을 갱신하고 SOUL.md를 재작성한다 (§R2)
    Rebuild {
        /// 파생 테이블을 비우고 전량 재구축한다. **임베딩 캐시는 보존한다** (T2)
        #[arg(long = "from-scratch")]
        from_scratch: bool,
        /// 임베딩 캐시 미스 시 에러 종료 (§R3, T28)
        #[arg(long)]
        offline: bool,
    },

    /// API 재호출로 새 ingest를 만든다. 기존 관측은 불변 (§R9, T16)
    Reanalyze { obs_id: ObsId },

    /// 관측 통계. `prompt_sha256` 경계를 반드시 출력한다 (§R11)
    Stats {
        /// `derived::stats::Stats`를 정준 JSON으로 출력한다 (§R6)
        #[arg(long)]
        json: bool,
    },

    /// 로컬 MCP 서버 (§19.7). 같은 디렉토리의 `soul-mcp`를 exec 한다
    Mcp {
        /// 에이전트 클라이언트에 붙일 설정 JSON을 출력한다. **파일을 고치지 않는다** (T38)
        #[arg(long = "print-config")]
        print_config: bool,
    },

    /// 축소 경로 (§19.8). exports/SOUL.prompt.md 에 쓰고 stdout으로도 낸다
    Export {
        #[arg(long, value_enum)]
        target: ExportTarget,
    },

    /// git gc 등 정리 작업 (§20.6)
    Maintain,

    /// 에이전트 트레이스 (§11.3)
    Trace {
        #[command(subcommand)]
        command: TraceCommand,
    },

    /// API 키 (§2). 값은 **OS 키체인**에만 저장하며 어디에도 출력하지 않는다
    Secrets {
        #[command(subcommand)]
        command: SecretsCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum SecretsCommand {
    /// stdin에서 값을 읽어 저장한다. 셸 히스토리에 키를 남기지 않으려고 인자로 받지 않는다
    Set {
        #[arg(value_name = "openai|search|youtube")]
        name: String,
    },
    /// 설정 여부만 보여준다. 값은 출력하지 않는다
    Status,
    Delete {
        #[arg(value_name = "openai|search|youtube")]
        name: String,
    },
    /// 환경변수(OPENAI_API_KEY 등)를 키체인으로 옮긴다 — `scripts/setup.sh`가 쓴다
    ImportEnv,
}

#[derive(Debug, Subcommand)]
pub enum TraceCommand {
    /// runs/ 전체 삭제. 관측과 SOUL.md는 건드리지 않는다 (T71)
    Purge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ExportTarget {
    /// SOUL.md 원문에서 마커 주석만 제거한 것 (§19.8)
    Prompt,
}

/// `yes` | `no` 뿐이다. 중간값을 추가하지 않는다 (§6.3, T56·T59).
fn parse_verdict(s: &str) -> std::result::Result<Verdict, String> {
    Verdict::parse(s).ok_or_else(|| format!("verdict는 yes 또는 no 여야 합니다 (받은 값: {s})"))
}

fn parse_kind(s: &str) -> std::result::Result<Kind, String> {
    Kind::parse(s)
        .ok_or_else(|| format!("kind는 text|image|audio|video 여야 합니다 (받은 값: {s})"))
}

/// 진입점. **에러를 여기서 소비한다** — 한국어 한 줄로 stderr에 적고 1로 끝난다 (§15).
pub fn run() -> anyhow::Result<()> {
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            // `--help`·`--version`은 실패가 아니다. 그 외 사용법 오류는 clap 기본값(2)이
            // 아니라 1로 끝낸다 — 이 앱의 실패 코드는 하나다.
            //
            // `DisplayHelpOnMissingArgumentOrSubcommand`는 **여기에 넣지 않는다.**
            // 인자 없이 `soul`만 친 경우이고, 도움말을 보여주더라도 명령을 수행하지
            // 못했으므로 실패다. 0으로 끝내면 스크립트가 오타를 성공으로 읽는다.
            let asked_for_help = matches!(
                e.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            );
            let _ = e.print();
            std::process::exit(if asked_for_help { 0 } else { 1 });
        }
    };

    match dispatch(cli.command) {
        Ok(()) => Ok(()),
        Err(e) => {
            eprintln!("오류: {e:#}");
            std::process::exit(1);
        }
    }
}

fn dispatch(command: Command) -> anyhow::Result<()> {
    match command {
        // §19.7 — 이 경로만은 앱 데이터 루트도 tokio 런타임도 만들지 않는다.
        // `--print-config`는 파일을 하나도 건드리지 않는다 (T38).
        Command::Mcp { print_config } => {
            if print_config {
                mcp::print_config()
            } else {
                mcp::exec_server()
            }
        }

        Command::Doctor { probe } => block_on(pipeline::doctor(Paths::discover()?, probe))?,
        Command::Ingest { target } => block_on(pipeline::ingest(Paths::discover()?, target))?,
        Command::Read {
            obs_id,
            verdict,
            prose,
        } => block_on(pipeline::read(Paths::discover()?, obs_id, verdict, prose))?,
        Command::Context { ingest_id, redo } => {
            block_on(pipeline::context(Paths::discover()?, ingest_id, redo))?
        }
        Command::Recast { ingest_id, kind } => {
            block_on(pipeline::recast(Paths::discover()?, ingest_id, kind))?
        }
        Command::Reflect { force } => block_on(pipeline::reflect(Paths::discover()?, force))?,
        Command::Reanalyze { obs_id } => block_on(pipeline::reanalyze(Paths::discover()?, obs_id))?,

        Command::Render { offline } => rebuild::render(&Paths::discover()?, offline),
        Command::Rebuild {
            from_scratch,
            offline,
        } => rebuild::rebuild(&Paths::discover()?, from_scratch, offline),

        Command::Stats { json } => stats::stats(&Paths::discover()?, json),
        Command::Export { target } => match target {
            ExportTarget::Prompt => export::export_prompt(&Paths::discover()?),
        },
        Command::Maintain => maintain::maintain(&Paths::discover()?),
        Command::Trace { command } => match command {
            TraceCommand::Purge => maintain::trace_purge(&Paths::discover()?),
        },

        // 키체인만 만진다 — 앱 데이터 루트도 런타임도 필요 없다.
        Command::Secrets { command } => match command {
            SecretsCommand::Set { name } => secrets::set(&name),
            SecretsCommand::Status => secrets::status(),
            SecretsCommand::Delete { name } => secrets::delete(&name),
            SecretsCommand::ImportEnv => secrets::import_env(),
        },
    }
}

/// 비동기 명령에서만 런타임을 만든다.
///
/// `#[tokio::main]`을 쓰면 `soul mcp`가 exec 하기 전에 워커 스레드를 띄우게 되고,
/// 그것은 §20.1의 "상주 프로세스 0개"와 §19.7의 취지 양쪽에 반한다.
pub(crate) fn block_on<F: std::future::Future>(fut: F) -> anyhow::Result<F::Output> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    Ok(rt.block_on(fut))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).unwrap_or_else(|e| panic!("{args:?} 파싱 실패: {e}"))
    }

    #[test]
    fn stdin_dash_is_a_value_not_a_flag() {
        // §14 — `soul ingest -` 는 stdin에서 텍스트를 읽는다.
        match parse(&["soul", "ingest", "-"]).command {
            Command::Ingest { target } => assert_eq!(target, "-"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn verdict_only_accepts_yes_and_no() {
        // T56 — yes/no 외의 값은 저장 경로에 들어가지도 못한다.
        let id = "01J8XQZK3M7P4RSTVWXYZ00001";
        match parse(&["soul", "read", id, "no", "사람이 나간 자리 같다"]).command {
            Command::Read {
                obs_id,
                verdict,
                prose,
            } => {
                assert_eq!(obs_id.as_str(), id);
                assert_eq!(verdict, Verdict::No);
                assert_eq!(prose.as_deref(), Some("사람이 나간 자리 같다"));
            }
            other => panic!("{other:?}"),
        }
        assert!(Cli::try_parse_from(["soul", "read", id, "maybe"]).is_err());
        assert!(Cli::try_parse_from(["soul", "read", "not-a-ulid", "yes"]).is_err());
    }

    #[test]
    fn offline_attaches_to_all_three_forms() {
        // §14 — "`--offline`은 세 형태 모두에 붙일 수 있으며".
        assert!(matches!(
            parse(&["soul", "render", "--offline"]).command,
            Command::Render { offline: true }
        ));
        assert!(matches!(
            parse(&["soul", "rebuild", "--offline"]).command,
            Command::Rebuild {
                from_scratch: false,
                offline: true
            }
        ));
        assert!(matches!(
            parse(&["soul", "rebuild", "--from-scratch", "--offline"]).command,
            Command::Rebuild {
                from_scratch: true,
                offline: true
            }
        ));
    }

    #[test]
    fn export_target_is_required_and_typed() {
        assert!(matches!(
            parse(&["soul", "export", "--target=prompt"]).command,
            Command::Export {
                target: ExportTarget::Prompt
            }
        ));
        assert!(Cli::try_parse_from(["soul", "export"]).is_err());
        assert!(Cli::try_parse_from(["soul", "export", "--target=soul.md"]).is_err());
    }

    #[test]
    fn trace_purge_is_a_nested_subcommand() {
        assert!(matches!(
            parse(&["soul", "trace", "purge"]).command,
            Command::Trace {
                command: TraceCommand::Purge
            }
        ));
        assert!(Cli::try_parse_from(["soul", "trace"]).is_err());
    }

    /// 실패 시 종료 코드는 1이다. `--help`·`--version`만 0이고, 서브커맨드 없이
    /// 부른 것은 도움말이 나오더라도 실패다.
    #[test]
    fn only_help_and_version_are_successes() {
        use clap::error::ErrorKind;
        let kind = |args: &[&str]| Cli::try_parse_from(args).unwrap_err().kind();
        assert_eq!(kind(&["soul", "--help"]), ErrorKind::DisplayHelp);
        assert_eq!(kind(&["soul", "--version"]), ErrorKind::DisplayVersion);
        assert_ne!(kind(&["soul"]), ErrorKind::DisplayHelp);
        assert_ne!(kind(&["soul"]), ErrorKind::DisplayVersion);
        assert_ne!(kind(&["soul", "nosuch"]), ErrorKind::DisplayHelp);
    }

    #[test]
    fn recast_kind_is_validated() {
        let id = "01J8XQZK3M7P4RSTVWXYZ00001";
        match parse(&["soul", "recast", id, "video"]).command {
            Command::Recast { kind, .. } => assert_eq!(kind, Kind::Video),
            other => panic!("{other:?}"),
        }
        assert!(Cli::try_parse_from(["soul", "recast", id, "gif"]).is_err());
    }
}
