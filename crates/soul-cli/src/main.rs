//! `soul` — 명령 표면 (§14).
//!
//! ```text
//! soul doctor                          # 키·모델 ID·ffmpeg 검증
//! soul ingest <path|url|->
//! soul read <obs_id> <verdict> [prose]
//! soul context <ingest_id> [--redo]    # 문화 글귀 재생성 (§9.10). 평시엔 자동
//! soul recast <ingest_id> <kind>       # YouTube 항목의 kind 뒤집기 (§9.3)
//! soul reflect [--force]
//! soul render
//! soul rebuild [--from-scratch] [--offline]
//! soul reanalyze <obs_id>
//! soul stats [--json]
//! soul mcp [--print-config]            # 로컬 MCP 서버
//! soul export --target=prompt          # 축소 경로 (§19.8)
//! soul maintain                        # git gc 등 정리 작업
//! soul trace purge                     # runs/ 전체 삭제
//! ```
//!
//! | 명령 | derived.sqlite | SOUL.md | 커밋 |
//! |---|---|---|---|
//! | `soul render` | 읽기만 | 재작성 | `render <T_ref>` |
//! | `soul rebuild` | 관측 재생으로 갱신 | 재작성 | `rebuild <n>` |
//! | `soul rebuild --from-scratch` | **삭제 후 전량 재구축** | 재작성 | `rebuild <n>` |

//!
//! 본체는 `lib.rs`에 있다 — 통합 테스트가 바이너리 크레이트의 모듈을 볼 수 없기 때문이다.
//! `cmd::run`이 에러를 직접 소비하고 실패 시 종료 코드 1로 끝낸다 (§15).

fn main() -> anyhow::Result<()> {
    soul_cli::cmd::run()
}
