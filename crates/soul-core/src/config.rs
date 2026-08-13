//! `config.toml` (§9.8 · §19.9).
//!
//! **모델 ID를 코드에 하드코딩하지 않는다.** 기본값이 빈 문자열이라는 사실이
//! §9.9 `soul doctor` 흐름의 전제다.

use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub api: ApiConfig,
    pub models: ModelsConfig,
    pub embed: EmbedConfig,
    pub privacy: PrivacyConfig,
    pub youtube: YoutubeConfig,
    pub search: SearchConfig,
    pub thresholds: Thresholds,
    pub local: LocalConfig,
    pub mcp: McpConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ApiConfig {
    /// 프록시·Azure 대응 (§9.8).
    pub base_url: String,
    pub timeout_secs: u64,
}

impl Default for ApiConfig {
    fn default() -> Self {
        ApiConfig {
            base_url: "https://api.openai.com/v1".into(),
            timeout_secs: 120,
        }
    }
}

/// 네 슬롯 전부 기본값이 **빈 문자열**이다 (§9.9).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ModelsConfig {
    /// Responses API, 이미지 입력.
    pub vision: String,
    /// Chat Completions, `input_audio`.
    pub audio: String,
    /// 텍스트·병합.
    pub text: String,
    /// 성찰 에이전트.
    pub reflect: String,
}

impl ModelsConfig {
    pub fn slot(&self, name: &str) -> Option<&str> {
        match name {
            "vision" => Some(&self.vision),
            "audio" => Some(&self.audio),
            "text" => Some(&self.text),
            "reflect" => Some(&self.reflect),
            _ => None,
        }
    }
    pub const SLOTS: [&'static str; 4] = ["vision", "audio", "text", "reflect"];
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct EmbedConfig {
    pub model: String,
    /// §20.2 — Matryoshka 절단. **단일 최대 레버다.** 1536으로 되돌리지 않는다.
    pub dims: usize,
}

impl Default for EmbedConfig {
    fn default() -> Self {
        EmbedConfig {
            model: "text-embedding-3-small".into(),
            dims: 256,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PrivacyConfig {
    /// §D7 — 첫 투입 전에 §D2 표를 그대로 보여주고 확인을 받는다.
    pub show_boundary_on_first_run: bool,
    /// 고지를 이미 확인했는지. 사용자가 확인하면 앱이 false로 내린다.
    pub boundary_acknowledged: bool,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        PrivacyConfig {
            show_boundary_on_first_run: true,
            boundary_acknowledged: false,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct YoutubeConfig {
    /// 선택. Data API v3 (§9.3).
    pub api_key: String,
    /// **기본값 false.** 이용약관 관련이므로 사용자가 명시적으로 켠다 (§9.3·§21-4).
    pub download_enabled: bool,
}

/// §11.1의 `web_search` 툴이 쓰는 제공자.
///
/// 명세는 검색 제공자를 지정하지 않았다 (§D6은 "검색 제공자"라고만 쓴다).
/// 키 없이 동작하는 것을 기본으로 두되, 사용자가 바꿀 수 있게 한다 — `docs/OPEN-DECISIONS.md` #14.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SearchConfig {
    /// `duckduckgo` | `brave` | `tavily`
    pub provider: String,
    pub api_key: String,
    pub max_results: usize,
}

impl Default for SearchConfig {
    fn default() -> Self {
        SearchConfig {
            provider: "duckduckgo".into(),
            api_key: String::new(),
            max_results: 8,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Thresholds {
    pub reflect_trigger_ingests: usize,
    pub axis_delta_max: f64,
    pub agent_max_turns_reflect: usize,
    pub agent_max_turns_critique: usize,
    pub agent_timeout_critique_secs: u64,
    /// false면 **문화 층 전체 비활성화** — 2×2의 절반을 잃는 대신 검색 노출이 사라진다 (§D6·§9.10).
    pub context_enabled: bool,
    pub critique_concurrency: usize,
    /// 3구간 합계 (§9.5).
    pub audio_cap_seconds: u32,
    pub video_max_seconds: u32,
    pub video_fps: u32,
    /// API가 거부하면 낮춘다 (§9.6).
    pub video_max_frames: u32,
    /// 긴 변 기준 (§9.4).
    pub image_max_edge_px: u32,
    /// §11.1 — 비평 에이전트의 `web_search` 호출 상한.
    pub agent_max_searches_critique: usize,
    /// §11.2 — `list_observations`의 limit 상한.
    pub list_observations_max: usize,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds {
            reflect_trigger_ingests: 20,
            axis_delta_max: 0.15,
            agent_max_turns_reflect: 12,
            agent_max_turns_critique: 8,
            agent_timeout_critique_secs: 90,
            context_enabled: true,
            critique_concurrency: 2,
            audio_cap_seconds: 30,
            video_max_seconds: 30,
            video_fps: 1,
            video_max_frames: 30,
            image_max_edge_px: 1280,
            agent_max_searches_critique: 3,
            list_observations_max: 200,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LocalConfig {
    /// §12.5.1 — 실루엣 O(n²) 회피.
    pub silhouette_max_samples: usize,
    pub thumb_max_edge_px: u32,
}

impl Default for LocalConfig {
    fn default() -> Self {
        LocalConfig {
            silhouette_max_samples: 500,
            thumb_max_edge_px: 256,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct McpConfig {
    pub enabled: bool,
    /// 한 번에 반환할 관측 상한 (§19.9).
    pub max_recall_limit: usize,
    /// 요약의 prose 절단 길이.
    pub prose_max_chars: usize,
}

impl Default for McpConfig {
    fn default() -> Self {
        McpConfig {
            enabled: true,
            max_recall_limit: 50,
            prose_max_chars: 200,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        if !path.exists() {
            return Ok(Config::default());
        }
        let s = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&s)?)
    }

    /// LF로 끝나는 UTF-8 파일로 쓴다.
    pub fn save(&self, path: &Path) -> Result<()> {
        let mut s = toml::to_string_pretty(self)?;
        if !s.ends_with('\n') {
            s.push('\n');
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, s)?;
        Ok(())
    }

    /// 모든 모델 슬롯이 비어 있으면 아무 투입도 처리할 수 없다 (§9.9).
    pub fn models_unset(&self) -> bool {
        ModelsConfig::SLOTS
            .iter()
            .all(|s| self.models.slot(s).map(|v| v.is_empty()).unwrap_or(true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_spec() {
        let c = Config::default();
        assert_eq!(c.embed.dims, 256);
        assert_eq!(c.embed.model, "text-embedding-3-small");
        assert_eq!(c.thresholds.reflect_trigger_ingests, 20);
        assert_eq!(c.thresholds.axis_delta_max, 0.15);
        assert_eq!(c.thresholds.critique_concurrency, 2);
        assert_eq!(c.thresholds.video_max_frames, 30);
        assert_eq!(c.thresholds.image_max_edge_px, 1280);
        assert_eq!(c.local.silhouette_max_samples, 500);
        assert_eq!(c.local.thumb_max_edge_px, 256);
        assert_eq!(c.mcp.max_recall_limit, 50);
        assert!(c.thresholds.context_enabled);
        assert!(!c.youtube.download_enabled, "§9.3 — 기본값은 꺼짐이다");
        assert!(c.privacy.show_boundary_on_first_run);
        assert!(c.models_unset(), "§9.9 — 모델 슬롯 기본값은 빈 문자열이다");
    }

    #[test]
    fn roundtrips_through_toml() {
        let c = Config::default();
        let s = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn partial_toml_fills_defaults() {
        let c: Config = toml::from_str("[embed]\ndims = 512\n").unwrap();
        assert_eq!(c.embed.dims, 512);
        assert_eq!(c.embed.model, "text-embedding-3-small");
        assert_eq!(c.thresholds.reflect_trigger_ingests, 20);
    }
}
