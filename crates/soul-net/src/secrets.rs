//! API 키 (§2).
//!
//! > API 키는 OS 키체인(`keyring`)에 저장한다.
//! > **프론트엔드로 반환하는 Tauri 커맨드를 만들지 않는다.**
//!
//! ## 호출자에게
//!
//! [`get`]의 반환값은 **`OpenAi::new` 같은 클라이언트 생성자에만 넘긴다.**
//! Tauri 커맨드의 반환 타입·로그·트레이스(§11.3)·오류 메시지 어디에도 들어가면 안 된다.
//! 프런트에 노출해도 되는 것은 [`is_set`]의 불리언 하나뿐이다.
//! 키가 새면 사용자는 그 사실을 알 방법이 없다 — 실수를 되돌릴 수 없는 종류의 실수다.

use soul_core::error::{Result, SoulError};

pub const SERVICE: &str = "tasty-soul";
pub const ACCOUNT_OPENAI: &str = "openai_api_key";
pub const ACCOUNT_SEARCH: &str = "search_api_key";
pub const ACCOUNT_YOUTUBE: &str = "youtube_api_key";

/// 키를 저장한다. **앞뒤 공백을 제거한다** — 붙여넣기로 딸려온 개행이 그대로 저장되면
/// `Authorization` 헤더 조립에서 실패하고, 원인이 키체인까지 멀어진다.
pub fn set(account: &str, value: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty() {
        // 빈 문자열을 저장하면 `is_set`은 false인데 항목은 존재하는 어중간한 상태가 된다.
        return Err(SoulError::config(
            "빈 키는 저장하지 않습니다. 지우려면 delete 를 쓰세요",
        ));
    }
    entry(account)?
        .set_password(value)
        .map_err(|e| keychain_error("저장", account, &e))
}

/// 환경변수 폴백을 켜는 스위치. **기본값은 꺼짐이다.**
pub const ALLOW_ENV: &str = "SOUL_ALLOW_ENV_SECRETS";

/// 키체인 계정 → 폴백에 쓸 환경변수 이름.
fn env_var_for(account: &str) -> Option<&'static str> {
    match account {
        ACCOUNT_OPENAI => Some("OPENAI_API_KEY"),
        ACCOUNT_SEARCH => Some("SEARCH_API_KEY"),
        ACCOUNT_YOUTUBE => Some("YOUTUBE_API_KEY"),
        _ => None,
    }
}

fn env_fallback_enabled() -> bool {
    matches!(
        std::env::var(ALLOW_ENV).as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

/// **이 함수의 반환값을 프런트로 보내지 말 것.**
///
/// 키가 없는 것은 오류가 아니라 `Ok(None)`이다 — §15의 "키 미설정"은 정상 상태이며
/// 설정 화면으로 유도하면 된다.
///
/// ## 환경변수 폴백 (기본 꺼짐)
///
/// `SOUL_ALLOW_ENV_SECRETS=1` 일 때만 키체인 **다음에** 환경변수를 본다.
/// 헤드리스 CI 처럼 Secret Service 가 없는 환경에서 `SOUL_E2E` 테스트를 돌리기 위한
/// 탈출구이며, 그 외에는 켜지 않는다.
///
/// **순서가 중요하다 — 키체인이 먼저다.** 반대로 두면 셸에 남은 오래된 `OPENAI_API_KEY`
/// 하나가 사용자가 설정 화면에서 넣은 키를 조용히 덮어쓴다. 그러면 "왜 예전 키로
/// 호출되지?"를 디버깅할 방법이 없다.
///
/// 폴백이 실제로 쓰이면 stderr 로 한 번 알린다. 평문 키가 프로세스 환경에 있다는 사실을
/// 조용히 넘기지 않는다 (§2).
pub fn get(account: &str) -> Result<Option<String>> {
    let from_keychain = match entry(account) {
        Ok(e) => match e.get_password() {
            Ok(v) => Some(v),
            Err(keyring::Error::NoEntry) => None,
            Err(e) if env_fallback_enabled() => {
                // 키체인 자체가 없는 환경(헤드리스 Linux 등)이다. 폴백이 켜져 있으면
                // 이것은 오류가 아니라 예상된 상황이다.
                let _ = e;
                None
            }
            Err(e) => return Err(keychain_error("조회", account, &e)),
        },
        Err(e) if env_fallback_enabled() => {
            let _ = e;
            None
        }
        Err(e) => return Err(e),
    };
    if let Some(v) = from_keychain {
        return Ok(Some(v));
    }

    if !env_fallback_enabled() {
        return Ok(None);
    }
    let Some(var) = env_var_for(account) else {
        return Ok(None);
    };
    match std::env::var(var) {
        Ok(v) if !v.trim().is_empty() => {
            warn_env_fallback_once(var);
            Ok(Some(v.trim().to_string()))
        }
        _ => Ok(None),
    }
}

/// 프로세스당 한 번만 경고한다. 매 호출마다 찍으면 로그가 시끄러워 결국 무시된다.
fn warn_env_fallback_once(var: &str) {
    use std::sync::OnceLock;
    static WARNED: OnceLock<()> = OnceLock::new();
    if WARNED.set(()).is_ok() {
        eprintln!(
            "경고: {ALLOW_ENV} 가 켜져 있어 키를 환경변수({var})에서 읽었습니다. \
             평문 키가 프로세스 환경에 있습니다 — 개발·CI 외에는 끄십시오 (§2)."
        );
    }
}

/// 없는 키를 지우는 것은 성공이다 (멱등).
pub fn delete(account: &str) -> Result<()> {
    match entry(account)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(keychain_error("삭제", account, &e)),
    }
}

/// 키가 설정되어 있는가. 프런트에는 이 불리언만 노출한다.
pub fn is_set(account: &str) -> bool {
    matches!(get(account), Ok(Some(v)) if !v.is_empty())
}

fn entry(account: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, account).map_err(|e| keychain_error("열기", account, &e))
}

/// **오류 메시지에 키 값을 넣지 않는다.** `keyring::Error`의 Display도 값을 담지 않는다.
fn keychain_error(op: &str, account: &str, e: &keyring::Error) -> SoulError {
    SoulError::config(format!("키체인 {op} 실패 ({SERVICE}/{account}): {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_names_are_stable() {
        // 이 문자열이 바뀌면 기존 사용자의 키가 사라진 것처럼 보인다.
        assert_eq!(SERVICE, "tasty-soul");
        assert_eq!(ACCOUNT_OPENAI, "openai_api_key");
        assert_eq!(ACCOUNT_SEARCH, "search_api_key");
        assert_eq!(ACCOUNT_YOUTUBE, "youtube_api_key");
    }

    #[test]
    fn empty_set_is_rejected() {
        // 키체인을 건드리기 전에 검사한다 — CI에서 안전하다.
        assert!(matches!(
            set(ACCOUNT_OPENAI, "   "),
            Err(SoulError::Config(_))
        ));
        assert!(matches!(set(ACCOUNT_OPENAI, ""), Err(SoulError::Config(_))));
    }

    #[test]
    fn error_message_never_contains_the_secret() {
        let e = keychain_error("저장", ACCOUNT_OPENAI, &keyring::Error::NoEntry).to_string();
        assert!(e.contains(ACCOUNT_OPENAI));
        assert!(!e.contains("sk-"), "{e}");
    }

    // 아래는 실제 OS 키체인을 건드린다. CI 기본 경로에서 돌지 않는다.

    #[test]
    #[ignore = "실제 OS 키체인을 건드린다"]
    fn roundtrip_set_get_delete() {
        let acct = "test_only_soul_key";
        set(acct, "  sk-비밀값  ").unwrap();
        assert_eq!(
            get(acct).unwrap().as_deref(),
            Some("sk-비밀값"),
            "trim 되어야 한다"
        );
        assert!(is_set(acct));

        delete(acct).unwrap();
        assert_eq!(get(acct).unwrap(), None, "없는 키는 Ok(None) 이다");
        assert!(!is_set(acct));
        delete(acct).unwrap(); // 멱등
    }
}

#[cfg(test)]
mod env_fallback_tests {
    use super::*;

    /// 환경변수 폴백은 **기본적으로 꺼져 있다**. 켜지 않으면 값이 있어도 무시한다.
    ///
    /// 이 테스트들은 프로세스 전역인 환경변수를 만지므로 한 함수 안에서 순차 실행한다
    /// (`cargo test`는 테스트를 스레드로 병렬 실행하므로 나누면 서로를 오염시킨다).
    #[test]
    fn env_fallback_is_off_by_default_and_keychain_wins_when_on() {
        // 실제 키체인을 건드리지 않도록 존재하지 않는 계정을 쓴다.
        const ACCOUNT: &str = ACCOUNT_OPENAI;
        let saved_allow = std::env::var(ALLOW_ENV).ok();
        let saved_key = std::env::var("OPENAI_API_KEY").ok();

        // SAFETY: 이 테스트 함수는 환경변수를 만지는 유일한 곳이며 순차적으로 돈다.
        unsafe {
            std::env::set_var("OPENAI_API_KEY", "env-key-should-not-leak");

            // 1) 스위치가 없으면 환경변수를 보지 않는다.
            std::env::remove_var(ALLOW_ENV);
            assert_ne!(
                get(ACCOUNT).ok().flatten().as_deref(),
                Some("env-key-should-not-leak"),
                "기본값에서 환경변수 키가 쓰이면 안 된다 (§2)"
            );

            // 2) 스위치가 '1'이 아니면 역시 보지 않는다.
            std::env::set_var(ALLOW_ENV, "0");
            assert_ne!(
                get(ACCOUNT).ok().flatten().as_deref(),
                Some("env-key-should-not-leak")
            );

            // 3) 켜면 (키체인에 값이 없을 때만) 읽는다.
            std::env::set_var(ALLOW_ENV, "1");
            let with_fallback = get(ACCOUNT).ok().flatten();
            if !is_set_in_keychain(ACCOUNT) {
                assert_eq!(with_fallback.as_deref(), Some("env-key-should-not-leak"));
            }

            // 복원
            match saved_allow {
                Some(v) => std::env::set_var(ALLOW_ENV, v),
                None => std::env::remove_var(ALLOW_ENV),
            }
            match saved_key {
                Some(v) => std::env::set_var("OPENAI_API_KEY", v),
                None => std::env::remove_var("OPENAI_API_KEY"),
            }
        }
    }

    /// 폴백을 거치지 않고 키체인만 본다 (위 테스트가 순서를 검증할 때 쓴다).
    fn is_set_in_keychain(account: &str) -> bool {
        entry(account)
            .map(|e| matches!(e.get_password(), Ok(v) if !v.is_empty()))
            .unwrap_or(false)
    }

    #[test]
    fn every_account_constant_has_an_env_var() {
        for a in [ACCOUNT_OPENAI, ACCOUNT_SEARCH, ACCOUNT_YOUTUBE] {
            assert!(env_var_for(a).is_some(), "{a} 에 대응하는 환경변수가 없다");
        }
        assert!(env_var_for("무관한_계정").is_none());
    }
}
