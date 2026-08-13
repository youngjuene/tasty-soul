//! 프롬프트 파일 로드와 `prompt_sha256` (§R11).
//!
//! > 프롬프트는 코드에 문자열로 박지 않고 앱 번들의 `prompts/` 아래 파일로 둔다.
//! > **렌더링이 끝난 최종 시스템 프롬프트의 sha256을 관측에 기록한다.**
//!
//! 이유: 프롬프트를 고치면 축 값과 서술의 분포가 통째로 바뀌므로, 기록이 없으면
//! 프롬프트 수정이 **가짜 드리프트**로 나타난다. 사용자는 자기 취향이 변했다고
//! 읽지만 실제로는 자가 변한 것이다. 이 오류는 눈에 띄지 않고 되돌릴 수도 없다 (§18-1, T23).
//!
//! **프롬프트 파일도 UTF-8·BOM 없음·LF로 고정한다** (T24c). 개행 문자만 바뀌어도
//! `prompt_sha256`이 달라져 있지도 않은 경계가 생긴다.

use crate::error::{Result, SoulError};
use crate::paths;
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptFile {
    /// §10.1 서술 생성 (이미지·오디오·텍스트 공통).
    Describe,
    /// §10.2 오디오 전용 추가 지시.
    Audio,
    /// §10.3 병합 호출 (영상 경로 전용).
    Merge,
    /// §10.4 비평 생성.
    Critique,
    /// §10.5 성찰 에이전트.
    Reflect,
}

impl PromptFile {
    pub fn file_name(&self) -> &'static str {
        match self {
            PromptFile::Describe => "describe.md",
            PromptFile::Audio => "audio.md",
            PromptFile::Merge => "merge.md",
            PromptFile::Critique => "critique.md",
            PromptFile::Reflect => "reflect.md",
        }
    }
    pub const ALL: [PromptFile; 5] = [
        PromptFile::Describe,
        PromptFile::Audio,
        PromptFile::Merge,
        PromptFile::Critique,
        PromptFile::Reflect,
    ];
}

/// 파일 내용을 읽는다. BOM이나 CRLF가 있으면 **에러다** — 조용히 고치지 않는다.
pub fn load(which: PromptFile) -> Result<String> {
    let dir = paths::prompts_dir().ok_or_else(|| {
        SoulError::config(
            "프롬프트 디렉토리(prompts/)를 찾을 수 없습니다. SOUL_PROMPTS_DIR를 확인하세요",
        )
    })?;
    let path = dir.join(which.file_name());
    if !path.is_file() {
        return Err(SoulError::MissingPath(path));
    }
    decode_strict(&std::fs::read(&path)?, &path)
}

/// **렌더링이 끝난 최종 시스템 프롬프트**의 sha256 hex 전문 (§R11).
/// 관측에 기록하는 값은 이 함수의 결과다.
pub fn sha256_of(final_prompt: &str) -> String {
    hex::encode(Sha256::digest(final_prompt.as_bytes()))
}

/// UTF-8·BOM 없음·LF를 강제한다 (T24c).
///
/// **고쳐서 통과시키지 않는다.** BOM을 벗기거나 CRLF를 LF로 바꾸면 그 시점에
/// `prompt_sha256`이 조용히 갈라져, 있지도 않은 프롬프트 경계가 관측에 생긴다 (§R11).
fn decode_strict(bytes: &[u8], path: &Path) -> Result<String> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return Err(SoulError::invalid(format!(
            "{}: BOM이 있습니다. 프롬프트 파일은 UTF-8·BOM 없음이어야 합니다 (§R11, T24c)",
            path.display()
        )));
    }
    // CRLF든 단독 CR이든 LF가 아닌 개행은 전부 거부한다.
    if let Some(i) = bytes.iter().position(|b| *b == b'\r') {
        let line = bytes[..i].iter().filter(|b| **b == b'\n').count() + 1;
        return Err(SoulError::invalid(format!(
            "{}: {}행에 CR이 있습니다. 프롬프트 파일의 개행은 LF만 허용합니다 (§R11, T24c)",
            path.display(),
            line
        )));
    }
    String::from_utf8(bytes.to_vec()).map_err(|e| {
        SoulError::invalid(format!(
            "{}: UTF-8이 아닙니다 ({}번째 바이트) (§R11, T24c)",
            path.display(),
            e.utf8_error().valid_up_to()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T24c — 실제 `prompts/*.md`가 UTF-8·BOM 없음·LF다.
    #[test]
    fn bundled_prompt_files_are_utf8_no_bom_lf() {
        let dir = paths::prompts_dir().expect("워크스페이스의 prompts/ 를 찾아야 한다");
        let mut checked = 0usize;
        for entry in std::fs::read_dir(&dir).expect("prompts/ 를 읽을 수 있어야 한다") {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let bytes = std::fs::read(&path).unwrap();
            decode_strict(&bytes, &path).unwrap_or_else(|e| panic!("{}", e));
            checked += 1;
        }
        assert!(
            checked >= PromptFile::ALL.len(),
            "검사한 프롬프트 파일이 너무 적다: {checked}"
        );
    }

    /// §R11 — 다섯 프롬프트 파일이 전부 존재하고 로드된다.
    #[test]
    fn every_prompt_file_loads_and_is_not_empty() {
        for which in PromptFile::ALL {
            let text = load(which).unwrap_or_else(|e| panic!("{}: {e}", which.file_name()));
            assert!(
                !text.trim().is_empty(),
                "{} 가 비어 있다",
                which.file_name()
            );
            assert!(!text.contains('\r'), "{} 에 CR이 있다", which.file_name());
        }
    }

    #[test]
    fn bom_is_rejected_not_stripped() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice("서술을 생성한다\n".as_bytes());
        let err = decode_strict(&bytes, Path::new("describe.md")).unwrap_err();
        assert!(format!("{err}").contains("BOM"), "{err}");
    }

    #[test]
    fn crlf_is_rejected_not_normalized() {
        let err = decode_strict(b"a\nb\r\nc\n", Path::new("describe.md")).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("CR"), "{msg}");
        assert!(msg.contains("2행"), "몇 행인지 알려야 한다: {msg}");
    }

    #[test]
    fn lone_cr_is_rejected_too() {
        assert!(decode_strict(b"a\rb", Path::new("x.md")).is_err());
    }

    #[test]
    fn invalid_utf8_is_rejected() {
        assert!(decode_strict(&[0x41, 0xFF, 0x42], Path::new("x.md")).is_err());
    }

    #[test]
    fn plain_lf_utf8_passes() {
        let s = decode_strict("한국어 산문\n두 번째 줄\n".as_bytes(), Path::new("x.md")).unwrap();
        assert_eq!(s, "한국어 산문\n두 번째 줄\n");
    }

    #[test]
    fn sha256_is_full_64_hex() {
        // 알려진 벡터로 고정한다 — 전문(64자)이며 잘라 쓰지 않는다.
        assert_eq!(
            sha256_of(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_of("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let h = sha256_of("렌더링이 끝난 최종 시스템 프롬프트");
        assert_eq!(h.len(), 64);
        assert!(h
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    /// §R11의 이유 — 개행 하나만 달라져도 해시가 갈린다(= 가짜 경계가 생긴다).
    #[test]
    fn newline_difference_changes_sha() {
        assert_ne!(sha256_of("a\nb"), sha256_of("a\r\nb"));
        assert_ne!(sha256_of("a"), sha256_of("a\n"));
    }
}
