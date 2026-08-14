/**
 * 공유 타입 — `src-tauri/src/commands.rs` 와 `crates/soul-core` 의 serde 표현을 그대로 미러링한다.
 *
 * ## 규칙
 * - 필드명은 **Rust 필드명 그대로**다. serde `rename` 이 없으므로 snake_case 를 유지한다.
 * - `Option<T>` → `T | null`. `undefined` 로 쓰지 않는다.
 * - `Vec<(A, B)>` → `[A, B][]`.
 * - `BTreeMap<String, f64>` → `Record<string, number>`.
 * - 여기에는 **판단이 없다.** 값은 전부 Rust 가 준 것을 그대로 담는다 (§2).
 */

// ───────────────────────────────────────────────────────── 열거값 (§6.2 · §6.3)

/** §6.2 — `Kind`. serde `rename_all = "lowercase"`. */
export const KINDS = ["text", "image", "audio", "video"] as const;
export type Kind = (typeof KINDS)[number];

/** §6.2 — `Quality`. **모델이 아니라 파이프라인이 정한다.** */
export const QUALITIES = ["full", "partial", "minimal"] as const;
export type Quality = (typeof QUALITIES)[number];

/**
 * §6.3 — **`yes` | `no` 뿐이다.**
 * 중간 선택지를 추가하지 말 것 (§18-6 · T56 · T59). 망설이는 항목은 카드를 넘긴다.
 */
export const VERDICTS = ["yes", "no"] as const;
export type Verdict = (typeof VERDICTS)[number];

/** §6.3 — 응답이 걸리는 층. `cultural` 의 target 은 `ingest` 가 아니라 `context` 다. */
export const LAYERS = ["sensory", "cultural"] as const;
export type Layer = (typeof LAYERS)[number];

/** §7 — 8축. **이 순서가 곧 `SOUL.md` 표의 행 순서이자 `axes_change` 의 인덱스 순서다** (§8.2.1). */
export const AXES = [
  "chroma",
  "luminance",
  "density",
  "grain",
  "tempo",
  "space",
  "valence",
  "intensity",
] as const;
export type AxisName = (typeof AXES)[number];

/** §12.6 — 2×2 셀. serde `rename_all = "snake_case"`. */
export const CELLS = ["read", "other_reason", "wrong_words", "unread"] as const;
export type CellName = (typeof CELLS)[number];

// ─────────────────────────────────────────── soul-core 미러 (§7 · §12)

/** §6.2 — 8축 값. 전부 필수, 각 `[0,1]`. */
export interface Axes {
  chroma: number;
  luminance: number;
  density: number;
  grain: number;
  tempo: number;
  space: number;
  valence: number;
  intensity: number;
}

/** §12.6 — 셀 개수. */
export interface CellCounts {
  read: number;
  other_reason: number;
  wrong_words: number;
  unread: number;
}

/** §12.6 — 방향의 일관성. 창은 전체 기간이다. */
export interface Coherence {
  /** 0 = 무작위, 1 = 완전 일관. */
  value: number;
  /** `prose` 가 있는 reading 수. */
  sample: number;
  /** 대표 사례 3건의 **target** ID. */
  examples: string[];
  /** `value >= 0.4 && sample >= 5`. Rust 가 판정한 값이다 — 프런트가 다시 재지 않는다. */
  systematic: boolean;
}

/** §12.5 `state_at(month)` 의 반환값. `month` 는 `"2026-06"` 형식이다. */
export interface MonthState {
  month: string;
  /** 직전 달 centroid 와의 코사인 거리. 어느 쪽이든 3건 미만이면 `null`. */
  drift: number | null;
  /** 누적 집합 군집의 실루엣 계수 평균. 군집이 없으면 `null`. */
  crystal: number | null;
  /** 이 달 말까지의 누적 ingest 수. */
  n: number;
}

/** §12.2 — 축별 90일 변화. 길이 8, 순서는 `AXES` 와 같다. 표본 부족 축은 `null`. */
export type AxesChange = [
  number | null,
  number | null,
  number | null,
  number | null,
  number | null,
  number | null,
  number | null,
  number | null,
];

/**
 * §R11 — "이 관측부터 프롬프트가 달라졌다"는 지점 하나.
 *
 * `kind`가 붙어 있는 이유: `ingest`는 `describe.md`의 해시를, `soul_delta`는
 * `reflect.md`의 해시를 들고 있다. 서로 비교할 수 있는 값이 아니므로 종류를 잃으면
 * 사용자가 두 구분선을 같은 축으로 읽는다.
 */
export interface PromptBoundary {
  id: string;
  kind: "ingest" | "soul_delta";
  sha256: string;
}

/**
 * §12 — `SOUL.md` 와 대시보드가 읽는 파생값 전부.
 *
 * **읽기만 한다.** 대시보드는 여기 없는 값을 그리지 않고, 없는 값을 0으로 채워
 * 가짜 차트를 만들지 않는다 (§13 화면 5).
 */
export interface Derived {
  /** §R1 — 시간 기준점 (`2026-08-13T09:12:33.123Z`). 관측이 없으면 `null`. */
  t_ref: string | null;
  t_first: string | null;
  /** supersede 되지 않은 ingest 수 (§R9). */
  observation_count: number;
  /** 전체 관측 수(모든 타입). */
  total_observation_count: number;

  /** §12.1 — `clamp(computed + offset, 0, 1)`. **화면에 표시하는 값은 이것이다.** */
  axes_final: Axes | null;
  axes_computed: Axes | null;
  axes_offset: Axes;
  axes_change: AxesChange;

  cells: CellCounts;
  /** 최근 30일 셀 추이 (2×2 격자의 추이 화살표용). */
  cells_recent30: CellCounts;
  /**
   * §8.2 `정정 N건` — `prose != null` 인 reading 수(두 층 합산).
   *
   * `coherence_*.sample` 로 대신 세면 안 된다. `|R| < 2` 일 때 `Coherence` 자체가
   * `null` 이라 정정이 실제로 있어도 0으로 보이고, 그것은 "정정한 적이 없다"는
   * 거짓말이 된다. 개수는 측정된 값이라 §R10 의 `—` 규칙 대상이 아니다.
   */
  corrections_total: number;
  /** §12.6 — 대시보드에는 싣지 않는다. 화면 5는 2×2 격자만 둔다. */
  divergence_sensory: number | null;
  divergence_cultural: number | null;
  coherence_sensory: Coherence | null;
  coherence_cultural: Coherence | null;

  /**
   * §12.5 — 월별 상태. `M` 전체.
   *
   * **지금 이 값을 그리는 화면이 없다.** 대시보드가 `(drift, crystal)` 경로로
   * 잇던 패널을 뺐다. 백엔드 계약이므로 타입은 그대로 둔다.
   */
  timeline: MonthState[];
  crystal_now: number | null;
  misread_ratio: number | null;

  /**
   * §R11 — `prompt_sha256`이 바뀌는 지점.
   *
   * 시간 축 위의 세로 구분선은 사라졌고(그 패널을 뺐다), 지금은 **개수만** 쓴다 —
   * 축 패널의 경고가 "질문지가 N번 바뀌었다"고 말할 때 그 N이다.
   */
  prompt_boundaries: PromptBoundary[];
}

// ───────────────────────────────────────────────── config.toml (§9.8 · §19.9)

export interface ApiConfig {
  base_url: string;
  timeout_secs: number;
}

/** §9.9 — 네 슬롯 전부 기본값이 **빈 문자열**이다. */
export interface ModelsConfig {
  vision: string;
  audio: string;
  text: string;
  reflect: string;
}

export const MODEL_SLOTS = ["vision", "audio", "text", "reflect"] as const;
export type ModelSlot = (typeof MODEL_SLOTS)[number];

export interface EmbedConfig {
  model: string;
  /** §20.2 — Matryoshka 절단. 1536으로 되돌리지 않는다. */
  dims: number;
}

export interface PrivacyConfig {
  show_boundary_on_first_run: boolean;
  boundary_acknowledged: boolean;
}

export interface YoutubeConfig {
  api_key: string;
  /** §9.3 · §21-4 — 기본값 false. 사용자가 명시적으로 켠다. */
  download_enabled: boolean;
}

export interface SearchConfig {
  /** `duckduckgo` | `brave` | `tavily` */
  provider: string;
  api_key: string;
  max_results: number;
}

export interface Thresholds {
  reflect_trigger_ingests: number;
  axis_delta_max: number;
  agent_max_turns_reflect: number;
  agent_max_turns_critique: number;
  agent_timeout_critique_secs: number;
  /** §D6 · §9.10 — false 면 문화 층 전체 비활성화. 2×2의 절반을 잃는다. */
  context_enabled: boolean;
  critique_concurrency: number;
  audio_cap_seconds: number;
  video_max_seconds: number;
  video_fps: number;
  video_max_frames: number;
  image_max_edge_px: number;
  agent_max_searches_critique: number;
  list_observations_max: number;
}

export interface LocalConfig {
  silhouette_max_samples: number;
  thumb_max_edge_px: number;
}

export interface McpConfig {
  enabled: boolean;
  max_recall_limit: number;
  prose_max_chars: number;
}

export interface Config {
  api: ApiConfig;
  models: ModelsConfig;
  embed: EmbedConfig;
  privacy: PrivacyConfig;
  youtube: YoutubeConfig;
  search: SearchConfig;
  thresholds: Thresholds;
  local: LocalConfig;
  mcp: McpConfig;
}

// ──────────────────────────────────────────────────────── doctor (§9.9)

export interface SlotCheck {
  slot: string;
  model: string;
  ok: boolean;
  error: string | null;
}

export interface DoctorReport {
  api_key_set: boolean;
  models_available: string[];
  slots: SlotCheck[];
  embed_ok: boolean | null;
  embed_error: string | null;
  /** 실행 파일 경로. 없으면 `null` → 조달 흐름 (§9.7). */
  ffmpeg: string | null;
  ffprobe: string | null;
  ytdlp: string | null;
  git_ok: boolean;
  soul_md_ok: boolean;
  /** §9.6 — 30장 호출이 가능한가. */
  multi_image_ok: boolean | null;
}

// ────────────────────────────────────────── commands.rs 미러 (§13)

/** §9.9 · §D7 — 앱 시작 시 가장 먼저 읽는 값. */
export interface SetupStatus {
  first_run: boolean;
  /** §D7 — 첫 투입 전에 §D2 표를 보여야 하는가. */
  needs_boundary_notice: boolean;
  api_key_set: boolean;
  models_unset: boolean;
  context_enabled: boolean;
}

/** `secrets_status()` 의 한 줄. `[계정, 설정되어 있는가]` — **값은 절대 오지 않는다** (§2). */
export type SecretStatus = [account: string, isSet: boolean];

/** §13 화면 2 — 감각 글귀 카드. 미응답 카드는 영속화하지 않는다. */
export interface SensoryCard {
  ingest_id: string;
  prose: string;
  thumb_data_url: string | null;
  kind: string;
  /** §9.3 — YouTube 추정 결과. true 면 한 탭으로 뒤집을 수 있다. */
  kind_is_guess: boolean;
}

export interface SourceLink {
  url: string;
  title: string;
}

/** §13 화면 2.1 — 문화 글귀 카드. 미응답 시에도 유지된다 (T53). */
export interface CulturalCard {
  context_id: string;
  ingest_id: string;
  critique: string;
  lineage: string[];
  /** false 면 카드 상단에 `NOT_GROUNDED_NOTICE` 를 표시한다 (T58). */
  grounded: boolean;
  sources: SourceLink[];
  thumb_data_url: string | null;
}

/** §8.3 규칙 7 — `gen_blocks_modified` 가 비어 있지 않으면 "재빌드 시 덮어써집니다". */
export interface SaveResult {
  profile_edits: number;
  commits: number;
  gen_blocks_modified: string[];
}

/** §13 화면 4 — 승인 diff. */
export interface ProposalView {
  /**
   * 좌우 diff **표시**용 전문. `soul:human` 이 들어 있다.
   *
   * 화면에 그리는 것은 §D4 대상이 아니다(목적지가 로컬 화면이다). 다만 **편집 상자에
   * 넣지 말 것** — 사용자가 고친 전문이 승인으로 돌아가면 `soul:human` 이
   * `soul_delta` 에 실리고, 그 뒤 성찰 호출마다 원격으로 나간다 (§18-4·T29).
   */
  current_text: string;
  proposed_text: string;
  /** 편집 상자가 바인딩하는 값 — `profile` 블록 본문**만**이다 (§D4). */
  current_profile_text: string;
  proposed_profile_text: string;
  /** 키는 `AxisName`. 없는 축은 키 자체가 없다 (§6.6). */
  axis_delta: Record<string, number>;
  cites: string[];
  rationale: string;
}

/**
 * §13 화면 6 — 아카이브 질의.
 *
 * **모든 필드가 필수다.** Rust 쪽 `ArchiveQuery` 는 `#[serde(default)]` 가 아니라
 * `#[derive(Default)]` 만 달려 있어, 빠진 키가 있으면 역직렬화가 실패한다.
 * 항상 `emptyArchiveQuery()` 로 시작해서 필요한 필드만 덮어써라.
 *
 * **어떤 필터도 API를 호출하지 않는다** (T68).
 */
export interface ArchiveQuery {
  kinds: string[];
  cells: string[];
  cluster: number | null;
  surprisal_min: number | null;
  surprisal_max: number | null;
  /** `"2026-06"` 형식. */
  months: string[];
  tags: string[];
  qualities: string[];
  /** 부분 문자열 일치만. **의미 검색은 없다.** */
  search: string | null;
  /** 산점도 가로축. `AxisName`. */
  x_axis: string | null;
  /** 산점도 세로축. `AxisName`. */
  y_axis: string | null;
}

/** 모든 키가 채워진 빈 질의. `archive_query` 는 이것을 기반으로 만든다. */
export function emptyArchiveQuery(): ArchiveQuery {
  return {
    kinds: [],
    cells: [],
    cluster: null,
    surprisal_min: null,
    surprisal_max: null,
    months: [],
    tags: [],
    qualities: [],
    search: null,
    x_axis: null,
    y_axis: null,
  };
}

/** §13 화면 6 — 산점도 타일 하나. */
export interface ArchiveItem {
  id: string;
  kind: string;
  /** 없으면 프런트가 `prose` 앞 40자를 렌더한다 (T70c · §20.4). */
  thumb_data_url: string | null;
  prose: string;
  tags: string[];
  surprisal: number;
  quality: string;
  /** `CellName` 또는 `null`(미완성 — 한 층이 미응답이거나 문화 글귀가 없다). */
  cell: string | null;
  cluster: number | null;
  /** `x_axis` / `y_axis` 좌표. **Rust 가 이미 계산해서 준 값이다.** */
  x: number;
  y: number;
  /** 구조 보기(PCA) 좌표. 없으면 구조 보기에서 제외한다. */
  px: number | null;
  py: number | null;
  month: string;
}

/** 항목 상세의 한 층. */
export interface ReadingView {
  /** `Verdict`. */
  verdict: string;
  prose: string | null;
  divergence: number | null;
}

/** §13 화면 6 — 항목 상세. 두 글귀를 나란히 놓는 것이 이 화면의 핵심이다. */
export interface ItemDetail {
  item: ArchiveItem;
  /** `source.origin` — `file://…` · watch URL · `clipboard:…` (§6.2). */
  origin: string;
  sensory_prose: string;
  sensory_reading: ReadingView | null;
  context: CulturalCard | null;
  cultural_reading: ReadingView | null;
  /** §9.10 — true 면 "문화 글귀 없음" 과 재시도 버튼을 둔다. */
  context_failed: boolean;
  /** §9.3 — true 면 kind 뒤집기 버튼을 둔다. */
  can_recast: boolean;
}

// ────────────────────────────────────────────────── UI 문구 (§13 · §12.6)

/**
 * §13 — 버튼 문구는 **층마다 다르다.** 명세에 적힌 문구를 그대로 쓴다.
 * 두 층 모두 **버튼은 정확히 2개**다 (§6.3 · T59).
 */
export const VERDICT_LABELS: Record<Layer, Record<Verdict, string>> = {
  sensory: { yes: "그렇다", no: "아니다" },
  cultural: { yes: "그래서 좋다", no: "그것 때문은 아니다" },
};

/** §13 화면 2.1 — `grounded === false` 일 때 카드 상단 문구 (T58). */
export const NOT_GROUNDED_NOTICE = "근거를 충분히 찾지 못했습니다";

/** §12.6 표의 "뜻" 열. */
export const CELL_MEANINGS: Record<CellName, string> = {
  read: "기계가 이 사람을 읽었다",
  other_reason: "보는 것은 같으나 끌리는 이유가 다르다",
  wrong_words: "서술은 빗나갔어도 무엇이 중요한지는 통한다",
  unread: "아직 못 잡았다",
};

/** §12.6 — `(감각, 문화)` 판정 조합. 셀 격자의 축 라벨에 쓴다. */
export const CELL_VERDICTS: Record<CellName, [sensory: Verdict, cultural: Verdict]> = {
  read: ["yes", "yes"],
  other_reason: ["yes", "no"],
  wrong_words: ["no", "yes"],
  unread: ["no", "no"],
};

/** §13 화면 6 — 셀 패싯의 다섯 번째 값. `ArchiveItem.cell === null` 에 해당한다. */
export const CELL_INCOMPLETE_LABEL = "미완성";

/** §7 표의 `0` / `1` 극 설명. 명세 문구 그대로다. */
export const AXIS_POLES: Record<AxisName, [low: string, high: string]> = {
  chroma: ["무채색에 가까움", "채도가 높고 색이 강함"],
  luminance: ["어둡고 그늘짐", "밝고 빛이 많음"],
  density: ["비어 있고 여백이 많음", "요소가 빽빽하게 들어참"],
  grain: ["매끄럽고 깨끗함", "거칠고 노이즈·질감이 두드러짐"],
  tempo: ["멈춰 있고 느림", "빠르고 급함"],
  space: ["평면적이고 가까움", "깊고 멀고 트여 있음"],
  valence: ["불안하고 서늘함", "안온하고 따뜻함"],
  intensity: ["은은하고 조용함", "강렬하고 압도적임"],
};

// ──────────────────────────────────────── 읽히는 이름 (§13 — 표시 전용)

/*
 * 화면에 **먼저** 나오는 우리말 이름. 식별자를 대체하지 않고 그 앞에 선다.
 *
 * `other_reason` · `surprisal` · `grain` 은 지울 수 없는 이름이다 — `SOUL.md` 와
 * MCP 와 CLI 가 그 이름으로 말하므로, 화면에서 없애 버리면 앱에서 그쪽으로 넘어가는
 * 순간 말이 끊긴다. 그렇다고 처음 열어 본 사람에게 `other_reason` 을 먼저 들이밀면
 * 그 사람은 아무것도 읽지 못한다.
 *
 * 그래서 **우리말을 앞에 놓고 식별자를 옆에 작게 남긴다.** 처음에는 우리말만 읽히고,
 * 오래 쓰면 옆에 있던 식별자가 저절로 눈에 익는다. 그때쯤 `SOUL.md` 를 열어도
 * 낯설지 않다.
 *
 * 뜻 전문은 각 화면의 `Explain` 이 접어서 들고 있다. 여기 있는 것은 **이름뿐**이다.
 */

/** 2×2 셀의 짧은 이름. 명세의 뜻 문장은 `CELL_MEANINGS` 에 그대로 있다 (§12.6). */
export const CELL_LABELS: Record<CellName, string> = {
  read: "제대로 읽었다",
  other_reason: "이유가 다르다",
  wrong_words: "말만 빗나갔다",
  unread: "아직 못 잡았다",
};

/** 8축의 우리말 이름. 양극의 뜻은 `AXIS_POLES` (§7 표 문구 그대로). */
export const AXIS_LABELS: Record<AxisName, string> = {
  chroma: "색감",
  luminance: "밝기",
  density: "밀도",
  grain: "거칠기",
  tempo: "빠르기",
  space: "공간감",
  valence: "편안함",
  intensity: "강렬함",
};

/** §6.2 `Kind`. */
export const KIND_LABELS: Record<Kind, string> = {
  text: "텍스트",
  image: "이미지",
  audio: "음악",
  video: "영상",
};

/** §6.2 `Quality` — **모델이 아니라 파이프라인이 정한다.** */
export const QUALITY_LABELS: Record<Quality, string> = {
  full: "온전히 봄",
  partial: "일부만 봄",
  minimal: "겉만 봄",
};

export const QUALITY_MEANINGS: Record<Quality, string> = {
  full: "볼 수 있는 것을 다 보고 서술했습니다.",
  partial: "일부만 보고 서술했습니다 — 예를 들어 영상의 앞 30초.",
  minimal: "제목과 썸네일 정도만 보고 서술했습니다.",
};

/** 모르는 값이 와도 화면이 비지 않게 원래 값을 그대로 돌려준다. */
export function kindLabel(v: string): string {
  return KIND_LABELS[v as Kind] ?? v;
}
export function qualityLabel(v: string): string {
  return QUALITY_LABELS[v as Quality] ?? v;
}
export function axisLabel(v: string): string {
  return AXIS_LABELS[v as AxisName] ?? v;
}
/** `null` 은 §12.6 의 다섯 번째 값(미완성)이다. */
export function cellLabel(v: string | null): string {
  if (v === null) return CELL_INCOMPLETE_LABEL;
  return CELL_LABELS[v as CellName] ?? v;
}

/**
 * 화면에 숫자로 나오는 말들의 뜻. **한 줄로 적는다** — 길면 아무도 읽지 않는다.
 *
 * `id` 는 같은 값이 `SOUL.md` · MCP · CLI 에서 불리는 이름이다. 설명을 펼친
 * 사람에게만 보이면 된다.
 */
export interface TermDef {
  name: string;
  id: string;
  gloss: string;
  /**
   * `SOUL.md` 가 같은 값을 **다른 우리말로** 부를 때 그 이름.
   *
   * 문서의 낱말은 §8.2 템플릿이라 고칠 수 없다 — T1(바이트 동일성)이 거기 걸려 있다.
   * 그렇다고 화면까지 `해상도` 로 두면 처음 온 사람은 화면 해상도를 떠올린다.
   * 그래서 **화면은 읽히는 말로 부르고, 두 이름을 여기서 이어 준다.** 이 다리가
   * 없으면 대시보드에서 익힌 말이 `SOUL.md` 에서 사라진 것처럼 보인다.
   */
  doc?: string;
}

export const TERMS = {
  surprisal: {
    name: "새로움",
    id: "surprisal",
    gloss: "이미 쌓인 것들과 얼마나 다른가. 1에 가까울수록 처음 보는 쪽입니다.",
  },
  cluster: {
    name: "무리",
    id: "cluster",
    gloss: "비슷한 것끼리 자동으로 묶은 덩어리. 번호에 순서나 좋고 나쁨은 없습니다.",
  },
  /*
    `drift` 는 여기 없다. 그 값을 그리던 대시보드 패널을 뺐고, `SOUL.md` 도
    `drift` 를 적지 않는다 — 축 표의 "90일 변화"는 `axes_change` 다.
    화면에 다시 나오면 그때 이름을 붙인다.
  */
  crystal: {
    name: "또렷함",
    id: "crystal",
    gloss: "비슷한 것끼리 얼마나 뚜렷하게 갈라지는가. 클수록 윤곽이 분명합니다.",
    doc: "해상도",
  },
  misread: {
    name: "어긋남",
    id: "misread_ratio",
    gloss: "기계의 서술을 '아니다'로 되돌린 비율입니다. 높다고 나쁜 것은 아닙니다.",
  },
  corrections: {
    name: "고쳐 쓴 것",
    id: "corrections_total",
    gloss: "«아니다»에 답하면서 직접 한 줄 써 넣은 횟수입니다. 두 카드를 합쳐 셉니다.",
    doc: "정정",
  },
  coherence: {
    name: "쏠림",
    id: "coherence",
    gloss:
      "고쳐 쓴 문장들이 한쪽으로 몰려 있는가. 1에 가까울수록 늘 같은 식으로 빗나갔다는 뜻입니다.",
    doc: "일관성",
  },
  divergence: {
    name: "벌어진 거리",
    id: "divergence",
    gloss: "최근 30일, 기계가 쓴 문장과 내가 고쳐 쓴 문장이 얼마나 멀었는지의 평균입니다.",
  },
  tRef: {
    name: "기준일",
    id: "T_ref",
    gloss: "모든 계산이 이 날을 기준으로 돕니다. 가장 마지막 기록의 날짜입니다.",
    doc: "기준",
  },
  count: {
    name: "넣은 것",
    id: "observation_count",
    gloss: "지금 세고 있는 항목 수. 다시 읽힌 항목은 새것 하나로만 셉니다.",
    doc: "관측",
  },
  first: {
    name: "시작",
    id: "t_first",
    gloss: "가장 처음 기록한 날짜입니다.",
    doc: "최초",
  },
  quality: {
    name: "기록 품질",
    id: "quality",
    gloss: "그 항목을 얼마나 들여다보고 서술했는지. 모델이 아니라 처리 과정이 정합니다.",
  },
  structure: {
    name: "구조 보기",
    id: "PCA",
    gloss: "비슷한 것끼리 가까이 모이도록 자동 배치합니다. 가로·세로 축 자체에는 뜻이 없습니다.",
  },
} as const satisfies Record<string, TermDef>;

/** §13 화면 6 — 산점도 기본 축 조합. */
export const DEFAULT_X_AXIS: AxisName = "grain";
export const DEFAULT_Y_AXIS: AxisName = "valence";

/** §13 화면 6 — 이 수를 넘으면 썸네일 대신 단색 점을 찍는다. */
export const TILE_RENDER_LIMIT = 200;

/** §20.4 · T70c — 썸네일이 없는 타일에 렌더할 서술문 길이. */
export const TILE_PROSE_CHARS = 40;

// ───────────────────────────────────────────────── 렌더 헬퍼 (§R10)

/** §R10 — `null` 파생값의 표기. 0으로 대체하거나 항목을 생략하지 않는다. */
export const EM_DASH = "—";

/**
 * 표시 자릿수로 반올림하고, 0이 되면 **부호를 뗀다.**
 *
 * `(-0.002882).toFixed(2)` 는 `"-0.00"` 이다. 읽는 사람에게는 서식 오류처럼 보이고,
 * 0에 가까운 값인데 "음수"라는 인상을 준다. 실루엣 계수(§12.5의 `crystal`)는 음수가
 * 될 수 있어 실제 데이터에서 나타난다 — 대시보드 헤더에 `해상도 -0.00` 으로 떴다.
 *
 * Rust 쪽 `soulmd::fmt_value` 도 같은 정규화를 한다. **두 곳이 어긋나면 같은 값이
 * `SOUL.md` 와 화면에서 다르게 보인다.**
 */
function roundForDisplay(v: number, digits: number): number {
  const f = 10 ** digits;
  const r = Math.round(v * f) / f;
  return r === 0 ? 0 : r;
}

/** 수치를 소수 `digits` 자리로 렌더한다. `null`/`undefined`/`NaN` 은 `—`. */
export function dash(v: number | null | undefined, digits = 2): string {
  if (v === null || v === undefined || !Number.isFinite(v)) return EM_DASH;
  return roundForDisplay(v, digits).toFixed(digits);
}

/** 변화량을 부호와 함께 렌더한다 (§8.2.1). `null` 은 `—`. */
export function dashSigned(v: number | null | undefined, digits = 2): string {
  if (v === null || v === undefined || !Number.isFinite(v)) return EM_DASH;
  const r = roundForDisplay(v, digits);
  // 0으로 반올림되면 부호는 의미가 없다. `−0.00` 을 내지 않는다.
  return (r < 0 ? "−" : "+") + Math.abs(r).toFixed(digits);
}

/**
 * 받침에 따라 조사를 고른다. `해상도라고` / `기준이라고`.
 *
 * 한글 음절은 `0xAC00` 부터 28개씩 묶여 있고, 그 안에서의 자리(`% 28`)가 종성이다.
 * 0이면 받침이 없다. 문자열을 붙여 만드는 문구는 이걸 안 하면 전부 `이라고` 로
 * 나가고, **그 순간 기계가 쓴 문장처럼 읽힌다.**
 *
 * 한글이 아닌 글자(영문 식별자 등)로 끝나면 받침 없는 쪽으로 둔다.
 */
export function josa(word: string, withBatchim: string, withoutBatchim: string): string {
  const last = word.trim().slice(-1);
  const code = last.charCodeAt(0);
  if (Number.isNaN(code) || code < 0xac00 || code > 0xd7a3) return withoutBatchim;
  return (code - 0xac00) % 28 === 0 ? withoutBatchim : withBatchim;
}

/** 문자열을 렌더한다. 비었거나 `null` 이면 `—`. */
export function dashText(v: string | null | undefined): string {
  if (v === null || v === undefined || v.trim() === "") return EM_DASH;
  return v;
}

/** `2026-08-13T09:12:33.123Z` → `2026-08-13` (UTC 기준, §8.2.1). */
export function dashDate(ts: string | null | undefined): string {
  if (!ts) return EM_DASH;
  const d = ts.slice(0, 10);
  return d.length === 10 ? d : EM_DASH;
}

// ────────────────────────────────────────── 화면 컴포넌트 계약

/**
 * `App.tsx` 가 import 하는 화면 컴포넌트의 공통 계약.
 *
 * **모든 화면은 named export 이며 props 를 받지 않는다.** 각 화면이 `lib/api.ts` 로
 * 직접 자기 데이터를 가져온다. 셸은 탭 전환과 §D7 고지 게이트만 맡는다.
 */
export type ScreenComponent = () => JSX.Element | null;

/** §D7 고지 화면만 예외적으로 콜백을 받는다. */
export interface BoundaryNoticeProps {
  /**
   * 사용자가 §D2 표를 확인하고 `acknowledge_boundary(context_enabled)` 가
   * 성공한 뒤에 부른다. 셸이 `setup_status()` 를 다시 읽고 본 화면으로 넘어간다.
   */
  onDone: () => void;
}
