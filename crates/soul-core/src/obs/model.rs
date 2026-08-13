//! 관측 로그 스키마 (§6).
//!
//! 파일 하나에 관측 하나, 파일명은 `<ULID>.json`, 디렉토리는 `observations/YYYY-MM/`.
//! **관측은 불변이다.** 한번 쓴 파일을 고치지 않는다 (§R2·§R9).

use crate::canon;
use crate::ids::ObsId;
use crate::time::Ts;
use crate::SCHEMA_VERSION;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ─────────────────────────────────────────────────────────────── 축 (§7)

/// §7의 8축. **순서가 곧 `SOUL.md` 표의 행 순서다** (§8.2.1).
pub const AXIS_NAMES: [&str; 8] = [
    "chroma",
    "luminance",
    "density",
    "grain",
    "tempo",
    "space",
    "valence",
    "intensity",
];

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Axis {
    Chroma,
    Luminance,
    Density,
    Grain,
    Tempo,
    Space,
    Valence,
    Intensity,
}

impl Axis {
    pub const ALL: [Axis; 8] = [
        Axis::Chroma,
        Axis::Luminance,
        Axis::Density,
        Axis::Grain,
        Axis::Tempo,
        Axis::Space,
        Axis::Valence,
        Axis::Intensity,
    ];

    pub fn name(&self) -> &'static str {
        AXIS_NAMES[*self as usize]
    }

    pub fn index(&self) -> usize {
        *self as usize
    }

    pub fn parse(s: &str) -> Option<Axis> {
        AXIS_NAMES
            .iter()
            .position(|n| *n == s)
            .map(|i| Axis::ALL[i])
    }
}

impl std::fmt::Display for Axis {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// 8축 값. 전부 필수, 각 `[0,1]` (§6.2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Axes {
    pub chroma: f64,
    pub luminance: f64,
    pub density: f64,
    pub grain: f64,
    pub tempo: f64,
    pub space: f64,
    pub valence: f64,
    pub intensity: f64,
}

impl Axes {
    pub const ZERO: Axes = Axes {
        chroma: 0.0,
        luminance: 0.0,
        density: 0.0,
        grain: 0.0,
        tempo: 0.0,
        space: 0.0,
        valence: 0.0,
        intensity: 0.0,
    };

    pub fn get(&self, a: Axis) -> f64 {
        match a {
            Axis::Chroma => self.chroma,
            Axis::Luminance => self.luminance,
            Axis::Density => self.density,
            Axis::Grain => self.grain,
            Axis::Tempo => self.tempo,
            Axis::Space => self.space,
            Axis::Valence => self.valence,
            Axis::Intensity => self.intensity,
        }
    }

    pub fn set(&mut self, a: Axis, v: f64) {
        match a {
            Axis::Chroma => self.chroma = v,
            Axis::Luminance => self.luminance = v,
            Axis::Density => self.density = v,
            Axis::Grain => self.grain = v,
            Axis::Tempo => self.tempo = v,
            Axis::Space => self.space = v,
            Axis::Valence => self.valence = v,
            Axis::Intensity => self.intensity = v,
        }
    }

    pub fn from_array(v: [f64; 8]) -> Axes {
        let mut a = Axes::ZERO;
        for (i, ax) in Axis::ALL.iter().enumerate() {
            a.set(*ax, v[i]);
        }
        a
    }

    pub fn to_array(&self) -> [f64; 8] {
        let mut out = [0.0; 8];
        for (i, ax) in Axis::ALL.iter().enumerate() {
            out[i] = self.get(*ax);
        }
        out
    }

    /// 모든 값이 `[0,1]`인지 검사한다. 모델 출력 검증에 쓴다 (§10.1).
    pub fn in_unit_range(&self) -> bool {
        self.to_array().iter().all(|v| (0.0..=1.0).contains(v))
    }
}

/// 8축 부분 델타 (§6.6 `axis_delta`). 없는 축은 키 자체가 없다.
pub type AxisDelta = BTreeMap<String, f64>;

// ─────────────────────────────────────────────────────── 열거형 (§6.2·§6.3)

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Text,
    Image,
    Audio,
    Video,
}

impl Kind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Image => "image",
            Kind::Audio => "audio",
            Kind::Video => "video",
        }
    }
    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "text" => Some(Kind::Text),
            "image" => Some(Kind::Image),
            "audio" => Some(Kind::Audio),
            "video" => Some(Kind::Video),
            _ => None,
        }
    }
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// §6.2 — **모델이 아니라 파이프라인이 정한다.**
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    Full,
    Partial,
    Minimal,
}

impl Quality {
    /// §12.1 가중치. 다른 곳에서 이 수를 다시 적지 않는다.
    pub fn weight(&self) -> f64 {
        match self {
            Quality::Full => 1.0,
            Quality::Partial => 0.6,
            Quality::Minimal => 0.2,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            Quality::Full => "full",
            Quality::Partial => "partial",
            Quality::Minimal => "minimal",
        }
    }
}

/// §6.3 — **`yes` | `no` 뿐이다.** 중간값을 추가하지 말 것 (§18-6, T56·T59).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Yes,
    No,
}

impl Verdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Verdict::Yes => "yes",
            Verdict::No => "no",
        }
    }
    pub fn parse(s: &str) -> Option<Verdict> {
        match s {
            "yes" => Some(Verdict::Yes),
            "no" => Some(Verdict::No),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layer {
    Sensory,
    Cultural,
}

impl Layer {
    pub fn as_str(&self) -> &'static str {
        match self {
            Layer::Sensory => "sensory",
            Layer::Cultural => "cultural",
        }
    }
    pub fn parse(s: &str) -> Option<Layer> {
        match s {
            "sensory" => Some(Layer::Sensory),
            "cultural" => Some(Layer::Cultural),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReactionAction {
    Starred,
    Revisited,
}

// ─────────────────────────────────────────────────────────── 공통 구조체

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub kind: Kind,
    pub sha256: String,
    /// 파일은 `file://` 절대경로, YouTube는 정규형 watch URL,
    /// 붙여넣은 텍스트는 `clipboard:<sha256 앞 12자>` (§6.2).
    pub origin: String,
    pub bytes: u64,
    /// **변환 결과가 아니라 원본 파일의 mime** (§9.4).
    pub mime: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Machine {
    pub prose: String,
    pub axes: Axes,
    /// 0~6개 한국어 명사구 (§6.2).
    pub tags: Vec<String>,
    pub quality: Quality,
    /// **렌더링이 끝난 최종 시스템 프롬프트의 sha256** (§R11).
    pub prompt_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelRef {
    pub provider: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_sha256: Option<String>,
    #[serde(default)]
    pub calls: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceRef {
    pub url: String,
    pub title: String,
    pub fetched_at: Ts,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Window {
    pub from: ObsId,
    pub to: ObsId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlockDelta {
    pub from_hash: String,
    pub to_text: String,
}

// ───────────────────────────────────────────────────────── 관측 타입 (§6)

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ingest {
    pub id: ObsId,
    pub ts: Ts,
    pub schema: u32,
    pub source: Source,
    pub machine: Machine,
    /// §12.4. 투입 시점 계산 후 **동결** (§R4).
    pub min_dist: Option<f64>,
    pub surprisal: f64,
    pub model: ModelRef,
    /// `soul reanalyze`/`soul recast`로 생성된 경우에만 존재 (§R9).
    /// **그 외에는 키를 넣지 않는다.**
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<ObsId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reading {
    pub id: ObsId,
    pub ts: Ts,
    pub schema: u32,
    pub layer: Layer,
    /// `sensory`면 `ingest` ID, `cultural`이면 **`context` ID** (§6.3).
    /// 이 2단 조인을 한 단으로 줄이면 셀이 영원히 null이다 (§12.6, T55c).
    pub target: ObsId,
    pub verdict: Verdict,
    /// `yes`면 반드시 `null` (§6.3, T7).
    pub prose: Option<String>,
    /// `prose`가 있을 때만 계산하는 **보조 지표**.
    pub divergence: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContextObs {
    pub id: ObsId,
    pub ts: Ts,
    pub schema: u32,
    pub target: ObsId,
    /// 한국어 산문 3~5문장. 목록·헤더·불릿 금지 (§6.4·§10.4).
    pub critique: String,
    pub lineage: Vec<String>,
    /// 실제로 사용한 검색어 전문. **프라이버시 고지의 근거** (§D6, T47).
    pub queries: Vec<String>,
    pub sources: Vec<SourceRef>,
    /// `len(sources) >= 2`. **모델에게 묻지 않고 파이프라인이 센다** (§6.4).
    pub grounded: bool,
    pub model: ModelRef,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reaction {
    pub id: ObsId,
    pub ts: Ts,
    pub schema: u32,
    pub target: ObsId,
    pub action: ReactionAction,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SoulDelta {
    pub id: ObsId,
    pub ts: Ts,
    pub schema: u32,
    pub window: Window,
    pub blocks: BTreeMap<String, BlockDelta>,
    pub axis_delta: AxisDelta,
    /// **항상 `null`로 기록한다.** 스키마 자리만 예약한다 (§6.6).
    pub morphology_delta: Option<serde_json::Value>,
    pub cites: Vec<ObsId>,
    pub rationale: String,
    pub model: ModelRef,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProfileEdit {
    pub id: ObsId,
    pub ts: Ts,
    pub schema: u32,
    pub block: String,
    pub from_hash: String,
    pub to_text: String,
    pub author: String,
}

// ─────────────────────────────────────────────────────────────── 합 타입

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Observation {
    Ingest(Ingest),
    Reading(Reading),
    Context(ContextObs),
    Reaction(Reaction),
    SoulDelta(SoulDelta),
    ProfileEdit(ProfileEdit),
}

impl Observation {
    pub fn id(&self) -> &ObsId {
        match self {
            Observation::Ingest(o) => &o.id,
            Observation::Reading(o) => &o.id,
            Observation::Context(o) => &o.id,
            Observation::Reaction(o) => &o.id,
            Observation::SoulDelta(o) => &o.id,
            Observation::ProfileEdit(o) => &o.id,
        }
    }

    pub fn ts(&self) -> Ts {
        match self {
            Observation::Ingest(o) => o.ts,
            Observation::Reading(o) => o.ts,
            Observation::Context(o) => o.ts,
            Observation::Reaction(o) => o.ts,
            Observation::SoulDelta(o) => o.ts,
            Observation::ProfileEdit(o) => o.ts,
        }
    }

    /// git 커밋 메시지의 `<type>` 부분 (§R8).
    pub fn type_name(&self) -> &'static str {
        match self {
            Observation::Ingest(_) => "ingest",
            Observation::Reading(_) => "reading",
            Observation::Context(_) => "context",
            Observation::Reaction(_) => "reaction",
            Observation::SoulDelta(_) => "soul_delta",
            Observation::ProfileEdit(_) => "profile_edit",
        }
    }

    /// §11.2 `list_observations`의 `prose_short` 원재료.
    /// `prose`가 없는 타입이면 `None`.
    pub fn prose(&self) -> Option<&str> {
        match self {
            Observation::Ingest(o) => Some(&o.machine.prose),
            Observation::Reading(o) => o.prose.as_deref(),
            Observation::Context(o) => Some(&o.critique),
            _ => None,
        }
    }

    pub fn as_ingest(&self) -> Option<&Ingest> {
        match self {
            Observation::Ingest(o) => Some(o),
            _ => None,
        }
    }
    pub fn as_reading(&self) -> Option<&Reading> {
        match self {
            Observation::Reading(o) => Some(o),
            _ => None,
        }
    }
    pub fn as_context(&self) -> Option<&ContextObs> {
        match self {
            Observation::Context(o) => Some(o),
            _ => None,
        }
    }
    pub fn as_soul_delta(&self) -> Option<&SoulDelta> {
        match self {
            Observation::SoulDelta(o) => Some(o),
            _ => None,
        }
    }
    pub fn as_profile_edit(&self) -> Option<&ProfileEdit> {
        match self {
            Observation::ProfileEdit(o) => Some(o),
            _ => None,
        }
    }

    /// §R6 정준 형식으로 직렬화한다. 관측 파일은 **이 함수로만** 쓴다.
    pub fn to_canonical_json(&self) -> crate::Result<String> {
        canon::to_string(self)
    }

    pub fn from_json(s: &str) -> crate::Result<Observation> {
        Ok(serde_json::from_str(s)?)
    }

    /// 구조 불변식을 검사한다. 기록 직전에 반드시 통과해야 한다.
    pub fn validate(&self) -> crate::Result<()> {
        use crate::error::SoulError;
        match self {
            Observation::Ingest(o) => {
                if !o.machine.axes.in_unit_range() {
                    return Err(SoulError::invalid("machine.axes 값이 [0,1] 밖입니다"));
                }
                if o.machine.tags.len() > 6 {
                    return Err(SoulError::invalid("machine.tags는 최대 6개입니다"));
                }
                if o.machine.prose.trim().is_empty() {
                    return Err(SoulError::invalid("machine.prose가 비어 있습니다"));
                }
            }
            Observation::Reading(o) => {
                // §6.3 — yes면 prose는 반드시 null (T7).
                if o.verdict == Verdict::Yes && o.prose.is_some() {
                    return Err(SoulError::invalid("verdict=yes 인데 prose가 있습니다"));
                }
                if o.prose.is_none() && o.divergence.is_some() {
                    return Err(SoulError::invalid("prose 없이 divergence를 둘 수 없습니다"));
                }
                if let Some(d) = o.divergence {
                    if !(0.0..=2.0).contains(&d) {
                        return Err(SoulError::invalid("divergence 범위를 벗어났습니다"));
                    }
                }
            }
            Observation::Context(o) => {
                // §18-7·T52 — 근거가 0건인 비평은 **기록 자체를 막는다.**
                //
                // 이 규칙은 원래 비평 에이전트의 `build_context` 한 곳에만 있었다. 함수 하나에
                // 둔 규칙은 새 경로(재분석·복구·수동 기록)가 생기는 순간 조용히 새어 나간다.
                // 근거 없는 비평이 한 건이라도 남으면 §12의 성찰이 그것을 사용자의 취향으로
                // 읽는다. 그래서 불변식으로 옮겼다 — 지우려면 §18-7부터 다시 읽어라.
                if o.sources.is_empty() {
                    return Err(SoulError::invalid(
                        "sources가 비어 있습니다 — 근거 없는 context는 기록하지 않습니다",
                    ));
                }
                // §D6 — `queries`가 프라이버시 고지("어떤 검색어가 나갔는지 남긴다")의 근거다.
                // 검색을 한 번도 하지 않고 만든 비평은 고지할 것이 없는 것이 아니라
                // **고지가 거짓인** 상태다 (T47). 빈 채로 남기느니 만들지 않는다.
                if o.queries.iter().all(|q| q.trim().is_empty()) {
                    return Err(SoulError::invalid(
                        "queries가 비어 있습니다 — 검색 기록 없는 context는 기록하지 않습니다",
                    ));
                }
                // §6.4 — grounded는 파이프라인이 센다.
                if o.grounded != (o.sources.len() >= 2) {
                    return Err(SoulError::invalid(
                        "grounded는 sources 개수(>=2)로만 결정됩니다",
                    ));
                }
                if o.lineage.len() > 4 {
                    return Err(SoulError::invalid("lineage는 최대 4개입니다"));
                }
            }
            Observation::SoulDelta(o) => {
                if o.morphology_delta.is_some() {
                    return Err(SoulError::invalid("morphology_delta는 항상 null입니다"));
                }
                for k in o.axis_delta.keys() {
                    if Axis::parse(k).is_none() {
                        return Err(SoulError::guardrail(format!("알 수 없는 축: {k}")));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// 새 관측의 공통 헤더를 만든다.
pub fn new_header() -> (ObsId, Ts, u32) {
    (crate::ids::new_id(), Ts::now(), SCHEMA_VERSION)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_ingest() -> Observation {
        Observation::Ingest(Ingest {
            id: ObsId::parse("01J8XQZK3M7P4RSTVWXYZ0").unwrap_or_else(|_| crate::ids::new_id()),
            ts: Ts::parse("2026-08-13T09:12:33.123Z").unwrap(),
            schema: SCHEMA_VERSION,
            source: Source {
                kind: Kind::Image,
                sha256: "abcd1234".into(),
                origin: "file:///Users/x/a.jpg".into(),
                bytes: 482913,
                mime: "image/jpeg".into(),
            },
            machine: Machine {
                prose: "차갑고 정돈된, 사람이 방금 지워진 실내".into(),
                axes: Axes {
                    chroma: 0.21,
                    luminance: 0.44,
                    density: 0.31,
                    grain: 0.68,
                    tempo: 0.15,
                    space: 0.77,
                    valence: 0.38,
                    intensity: 0.25,
                },
                tags: vec!["실내".into(), "자연광".into(), "무인".into()],
                quality: Quality::Full,
                prompt_sha256: "9f2c1a".into(),
            },
            min_dist: Some(0.42),
            surprisal: 0.83,
            model: ModelRef {
                provider: "openai".into(),
                id: "gpt-x".into(),
                prompt_sha256: None,
                calls: vec!["resp_abc123".into()],
            },
            supersedes: None,
        })
    }

    #[test]
    fn ingest_roundtrips_through_canonical_json() {
        let o = sample_ingest();
        let s = o.to_canonical_json().unwrap();
        assert!(s.starts_with("{\n  \"id\":"), "키가 정렬되어야 한다: {s}");
        assert!(
            !s.contains("supersedes"),
            "supersedes 없으면 키 자체가 없어야 한다"
        );
        assert!(s.contains("\"type\": \"ingest\""));
        let back = Observation::from_json(&s).unwrap();
        assert_eq!(o, back);
    }

    #[test]
    fn axis_order_matches_spec() {
        assert_eq!(
            Axis::ALL.map(|a| a.name()),
            [
                "chroma",
                "luminance",
                "density",
                "grain",
                "tempo",
                "space",
                "valence",
                "intensity"
            ]
        );
    }

    #[test]
    fn quality_weights_are_spec_values() {
        assert_eq!(Quality::Full.weight(), 1.0);
        assert_eq!(Quality::Partial.weight(), 0.6);
        assert_eq!(Quality::Minimal.weight(), 0.2);
    }

    #[test]
    fn yes_verdict_with_prose_is_rejected() {
        let r = Observation::Reading(Reading {
            id: crate::ids::new_id(),
            ts: Ts::now(),
            schema: SCHEMA_VERSION,
            layer: Layer::Sensory,
            target: crate::ids::new_id(),
            verdict: Verdict::Yes,
            prose: Some("x".into()),
            divergence: None,
        });
        assert!(r.validate().is_err());
    }

    #[test]
    fn grounded_must_match_source_count() {
        let c = Observation::Context(ContextObs {
            id: crate::ids::new_id(),
            ts: Ts::now(),
            schema: SCHEMA_VERSION,
            target: crate::ids::new_id(),
            critique: "x".into(),
            lineage: vec![],
            queries: vec!["q".into()],
            sources: vec![],
            grounded: true, // sources가 0인데 true → 거부
            model: ModelRef {
                provider: "openai".into(),
                id: "m".into(),
                prompt_sha256: None,
                calls: vec![],
            },
        });
        assert!(c.validate().is_err());
    }

    /// §6.4를 흔들지 않고 §18-7만 시험하려고 나머지는 다 채워 둔다.
    fn context_with(queries: Vec<String>, sources: Vec<SourceRef>) -> Observation {
        Observation::Context(ContextObs {
            id: crate::ids::new_id(),
            ts: Ts::now(),
            schema: SCHEMA_VERSION,
            target: crate::ids::new_id(),
            critique: "이 사진은 1970년대 무인 실내 사진의 어법을 따른다.".into(),
            lineage: vec![],
            grounded: sources.len() >= 2,
            queries,
            sources,
            model: ModelRef {
                provider: "openai".into(),
                id: "m".into(),
                prompt_sha256: None,
                calls: vec![],
            },
        })
    }

    fn src(url: &str) -> SourceRef {
        SourceRef {
            url: url.into(),
            title: "제목".into(),
            fetched_at: Ts::parse("2026-08-13T09:20:11.000Z").unwrap(),
        }
    }

    /// T52·§18-7 — 근거 0건인 `context`는 **불변식**으로 막는다.
    /// 함수 하나(`build_context`)의 규칙으로 두면 다음 경로가 생길 때 조용히 새어 나간다.
    #[test]
    fn context_without_sources_is_rejected() {
        assert!(context_with(vec!["질의".into()], vec![])
            .validate()
            .is_err());
    }

    /// §D6 — `queries`가 프라이버시 고지의 근거다. 비어 있으면 고지가 거짓이 된다 (T47).
    #[test]
    fn context_without_queries_is_rejected() {
        assert!(
            context_with(vec![], vec![src("https://a"), src("https://b")])
                .validate()
                .is_err()
        );
    }

    /// 위 두 규칙을 넣고도 §6.4의 `grounded` 검사는 그대로 살아 있어야 한다.
    #[test]
    fn grounded_must_still_match_source_count_when_both_fields_are_filled() {
        let mut o = context_with(
            vec!["질의".into()],
            vec![src("https://a"), src("https://b")],
        );
        o.validate().expect("2건 + grounded=true 는 통과한다");
        if let Observation::Context(c) = &mut o {
            c.grounded = false; // 2건인데 false → 거부
        }
        assert!(o.validate().is_err());
    }

    #[test]
    fn verdict_enum_rejects_third_value() {
        // T56 — yes/no 외의 값은 역직렬화되지 않는다.
        assert!(serde_json::from_str::<Verdict>("\"maybe\"").is_err());
        assert!(serde_json::from_str::<Verdict>("\"partial\"").is_err());
    }
}
