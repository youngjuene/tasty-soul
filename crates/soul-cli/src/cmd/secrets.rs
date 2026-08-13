//! `soul secrets` — API 키를 **OS 키체인에** 넣고 확인한다 (§2).
//!
//! ## 왜 파일이 아니라 키체인인가
//!
//! §2: *"API 키는 OS 키체인(`keyring`)에 저장한다. 프론트엔드로 반환하는 Tauri 커맨드를
//! 만들지 않는다."* 앱이 읽는 **유일한** 출처는 키체인이다.
//!
//! `.env`는 개발자 편의를 위한 **투입구**일 뿐이며, `scripts/setup.sh`가 그것을 읽어
//! 이 명령으로 키체인에 넣는다. 그 뒤로 앱은 `.env`를 쳐다보지 않는다.
//! 두 개의 키 저장소를 두면 어느 쪽이 진실인지 알 수 없게 되고, 평문 파일 쪽이
//! 조용히 이기게 된다.
//!
//! ## 값을 인자로 받지 않는다
//!
//! `soul secrets set openai sk-...` 는 셸 히스토리와 `ps` 출력에 키를 남긴다.
//! 그래서 **stdin으로만** 받는다.
//!
//! ```bash
//! printf '%s' "$OPENAI_API_KEY" | soul secrets set openai
//! soul secrets status
//! soul secrets delete openai
//! ```

use anyhow::{anyhow, Result};
use soul_net::secrets;
use std::io::Read;

/// 사용자에게 보이는 이름 → 키체인 계정 (§2).
const ACCOUNTS: [(&str, &str, &str); 3] = [
    (
        "openai",
        secrets::ACCOUNT_OPENAI,
        "OpenAI API 키 — 없으면 모든 투입 경로가 비활성화된다 (§15)",
    ),
    (
        "search",
        secrets::ACCOUNT_SEARCH,
        "검색 제공자 키 — provider가 duckduckgo면 필요 없다 (OPEN-DECISIONS #14)",
    ),
    (
        "youtube",
        secrets::ACCOUNT_YOUTUBE,
        "YouTube Data API v3 키 — 선택. 없으면 kind 추정이 video로 고정된다 (§9.3)",
    ),
];

fn resolve(name: &str) -> Result<&'static str> {
    ACCOUNTS
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, account, _)| *account)
        .ok_or_else(|| {
            let known: Vec<&str> = ACCOUNTS.iter().map(|(n, _, _)| *n).collect();
            anyhow!(
                "알 수 없는 키 이름: {name} (가능한 값: {})",
                known.join(" · ")
            )
        })
}

/// stdin에서 값을 읽어 키체인에 넣는다. **인자로 받지 않는다.**
pub fn set(name: &str) -> Result<()> {
    let account = resolve(name)?;
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw)?;
    // 붙여넣기·파이프에 딸려오는 개행을 없앤다. `secrets::set`도 trim 하지만
    // 여기서 빈 값을 먼저 걸러야 사용자에게 나은 메시지를 줄 수 있다.
    let value = raw.trim();
    if value.is_empty() {
        return Err(anyhow!(
            "빈 값입니다. 사용법: printf '%s' \"$OPENAI_API_KEY\" | soul secrets set {name}"
        ));
    }
    secrets::set(account, value)?;
    // **값을 출력하지 않는다.** 길이만 알려 붙여넣기 사고를 잡을 수 있게 한다.
    println!(
        "{name}: 키체인에 저장했습니다 ({}자)",
        value.chars().count()
    );
    Ok(())
}

/// 설정 여부만 보여준다. **값은 절대 출력하지 않는다** (§2).
pub fn status() -> Result<()> {
    let mut any = false;
    for (name, account, help) in ACCOUNTS {
        let set = secrets::is_set(account);
        any |= set;
        println!(
            "{:9} {}  {help}",
            name,
            if set { "설정됨" } else { "없음  " }
        );
    }
    if !any {
        eprintln!();
        eprintln!("키가 하나도 없습니다. `scripts/setup.sh` 또는:");
        eprintln!("  printf '%s' \"$OPENAI_API_KEY\" | soul secrets set openai");
    }
    Ok(())
}

pub fn delete(name: &str) -> Result<()> {
    let account = resolve(name)?;
    secrets::delete(account)?;
    println!("{name}: 삭제했습니다");
    Ok(())
}

/// `.env` 등에서 온 환경변수를 키체인으로 **한 번에 옮긴다** (`scripts/setup.sh`용).
///
/// 값이 비어 있는 변수는 건너뛴다 — `.env.example`을 그대로 복사한 상태에서
/// 빈 키가 저장되어 "설정됨"으로 보이는 것이 가장 나쁘다.
pub fn import_env() -> Result<()> {
    const ENV_VARS: [(&str, &str); 3] = [
        ("openai", "OPENAI_API_KEY"),
        ("search", "SEARCH_API_KEY"),
        ("youtube", "YOUTUBE_API_KEY"),
    ];
    let mut imported = 0usize;
    for (name, var) in ENV_VARS {
        let Ok(value) = std::env::var(var) else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        secrets::set(resolve(name)?, value)?;
        println!(
            "{name}: {var} 에서 키체인으로 옮겼습니다 ({}자)",
            value.chars().count()
        );
        imported += 1;
    }
    if imported == 0 {
        eprintln!(
            "옮길 키가 없습니다 (OPENAI_API_KEY · SEARCH_API_KEY · YOUTUBE_API_KEY 전부 비어 있음)"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_names_map_to_spec_constants() {
        assert_eq!(resolve("openai").unwrap(), secrets::ACCOUNT_OPENAI);
        assert_eq!(resolve("search").unwrap(), secrets::ACCOUNT_SEARCH);
        assert_eq!(resolve("youtube").unwrap(), secrets::ACCOUNT_YOUTUBE);
    }

    #[test]
    fn unknown_name_lists_the_valid_ones() {
        let e = resolve("anthropic").unwrap_err().to_string();
        assert!(e.contains("openai"), "{e}");
        assert!(e.contains("youtube"), "{e}");
    }
}
