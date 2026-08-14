/**
 * 디자인 미리보기용 고정 데이터.
 *
 * 실제 값의 **모양과 극단**을 보여주는 것이 목적이다. 예쁜 값만 넣으면
 * 디자인이 실제 데이터에서 깨진다. 그래서 아래를 일부러 섞어 두었다:
 *
 * - `null` 파생값 (§R10 — 화면에 `—` 로 나와야 한다)
 * - 두 층 중 하나만 답한 항목 (2×2 셀이 `null` 인 상태)
 * - 근거가 얇은 비평 (`grounded: false` → 경고가 떠야 한다)
 * - 아주 긴 서술문과 아주 짧은 서술문
 * - 240건 (200건 초과 → 아카이브가 점으로 그려야 한다, §13 화면 6)
 */

import type {
  ArchiveItem,
  ArchiveQuery,
  Config,
  CulturalCard,
  Derived,
  DoctorReport,
  ItemDetail,
  ProposalView,
  SensoryCard,
} from "../lib/types";

const AXES = [
  "chroma", "luminance", "density", "grain",
  "tempo", "space", "valence", "intensity",
] as const;

function axesFrom(base: number) {
  const o = {} as Record<(typeof AXES)[number], number>;
  AXES.forEach((a, i) => (o[a] = Math.round((((base + i * 0.09) % 1) + 1) % 1 * 100) / 100));
  return o;
}

/** 길이와 어조를 일부러 다르게 — 타이포가 실제로 견디는지 보려고. */
const PROSE = [
  "차갑고 정돈된, 사람이 방금 지워진 실내",
  "젖은 아스팔트가 신호등을 두 번 삼킨다",
  "빛이 바랜 여름 마당, 아무도 부르지 않는다",
  "소리가 벽을 통과하지 못하고 방 안에서만 늙는다",
  "겨울 아침의 부엌은 아직 아무도 깨우지 않았다. 냉장고만 혼자 낮게 울고 있고, 창틀에 맺힌 물기가 아직 마르지 않은 채로 남아 있다",
  "정적",
];

const KINDS = ["image", "text", "audio", "video"] as const;
const CELLS = ["read", "other_reason", "wrong_words", "unread", null] as const;
const QUAL = ["full", "partial", "minimal"] as const;
const TAGS = ["실내", "자연광", "무인", "젖음", "저채도", "새벽"];

export const items: ArchiveItem[] = Array.from({ length: 240 }, (_, i) => ({
  id: `01J8XQZK3M7P4RSTVWXY${String(i).padStart(2, "0")}`,
  kind: KINDS[i % 4],
  thumb_data_url: null,
  prose: PROSE[i % PROSE.length]!,
  tags: TAGS.slice(i % 4, (i % 4) + ((i % 3) + 1)),
  surprisal: Math.round((((i * 37) % 100) / 100) * 100) / 100,
  quality: QUAL[i % 3]!,
  cell: CELLS[i % 5] ?? null,
  cluster: i % 5,
  x: ((i * 17) % 100) / 100,
  y: ((i * 41) % 100) / 100,
  px: ((((i * 23) % 100) / 100) - 0.5) * 2,
  py: ((((i * 13) % 100) / 100) - 0.5) * 2,
  month: `2026-0${(i % 6) + 1}`,
}));

export const derived: Derived = {
  t_ref: "2026-08-13T09:12:33.123Z",
  t_first: "2026-03-02T10:00:00.000Z",
  observation_count: 342,
  total_observation_count: 901,
  axes_final: axesFrom(0.34),
  axes_computed: axesFrom(0.31),
  axes_offset: axesFrom(0.02),
  // 두 축이 `null` — §R10 대로 `—` 로 나와야 한다.
  axes_change: [null, 0.03, -0.06, 0.14, null, 0.02, -0.11, 0.05],
  cells: { read: 41, other_reason: 27, wrong_words: 12, unread: 9 },
  cells_recent30: { read: 12, other_reason: 11, wrong_words: 3, unread: 1 },
  corrections_total: 18,
  divergence_sensory: 0.31,
  divergence_cultural: 0.44,
  coherence_sensory: {
    value: 0.62,
    sample: 12,
    examples: [items[0]!.id, items[1]!.id, items[2]!.id],
    systematic: true,
  },
  coherence_cultural: { value: 0.44, sample: 6, examples: [], systematic: true },
  // 가운데 두 달이 `null` — 선이 끊기지 않아야 한다 (§13 화면 5).
  timeline: ["2026-03", "2026-04", "2026-05", "2026-06", "2026-07", "2026-08"].map((m, i) => ({
    month: m,
    drift: i === 2 ? null : Math.round((0.2 + i * 0.09) * 100) / 100,
    crystal: i === 1 ? null : Math.round((0.35 + i * 0.05) * 100) / 100,
    n: 40 + i * 55,
  })),
  crystal_now: 0.58,
  misread_ratio: 0.31,
  // §R11 — 대시보드가 시간 축에 세로 구분선을 긋는다.
  prompt_boundaries: [
    { id: items[0]!.id, kind: "ingest", sha256: "a".repeat(64) },
    { id: items[120]!.id, kind: "ingest", sha256: "c".repeat(64) },
    { id: items[200]!.id, kind: "soul_delta", sha256: "d".repeat(64) },
  ],
};

export const SOUL_MD = `# SOUL

<!-- soul:gen id=header -->
기준 2026-08-13 · 관측 342 · 최초 2026-03-02
어긋남 0.31 · 해상도 0.58
<!-- /soul:gen -->

## 지금의 취향

<!-- soul:neg id=profile rev=17 hash=a3f91c -->
인공물보다 방치된 것을 고른다. 채도가 아니라 습도로
공간을 읽고, 사람이 방금 나간 자리에 반복해서 멈춘다.
<!-- /soul:neg -->

## 축

<!-- soul:gen id=axes -->
| 축 | 값 | 90일 변화 |
|---|---|---|
| chroma | 0.34 | — |
| luminance | 0.52 | +0.03 |
| density | 0.28 | −0.06 |
| grain | 0.71 | +0.14 |
| tempo | 0.19 | — |
| space | 0.66 | +0.02 |
| valence | 0.41 | −0.11 |
| intensity | 0.30 | +0.05 |
<!-- /soul:gen -->

## 어긋남

<!-- soul:gen id=divergence -->
- 일관성 0.62 · 정정 18건
- 대표 사례: ${items[0]!.id} / ${items[1]!.id}
<!-- /soul:gen -->

## 내가 쓴 것

<!-- soul:human -->
비 온 다음 날 아침에만 찍게 된다. 왜인지는 모르겠다.
<!-- /soul:human -->
`;

export const sensoryCard: SensoryCard = {
  ingest_id: items[0]!.id,
  prose: PROSE[0]!,
  thumb_data_url: null,
  kind: "image",
  kind_is_guess: false,
};

/** YouTube 추정 항목 — 카드에 kind 뒤집기 버튼이 떠야 한다 (§9.3). */
export const guessCard: SensoryCard = {
  ingest_id: items[3]!.id,
  prose: PROSE[3]!,
  thumb_data_url: null,
  kind: "audio",
  kind_is_guess: true,
};

export const culturalCard: CulturalCard = {
  context_id: "01J8XQZN3M7P4RSTVWXYZ1",
  ingest_id: items[0]!.id,
  critique:
    "잔향을 악기처럼 다루던 90년대 초 영국 기타 음악의 어법을 따르되, 그 계보가 대개 " +
    "기댔던 벽 같은 볼륨 대신 여백을 남긴다. 소리의 크기가 아니라 소리가 사라지는 " +
    "속도로 공간을 만드는 쪽이다. 그래서 같은 계보 안에서도 유독 조용하게 들린다.",
  lineage: ["슈게이즈", "드림 팝"],
  grounded: true,
  sources: [
    { url: "https://example.invalid/a", title: "90년대 영국 기타 음악의 잔향 사용" },
    { url: "https://example.invalid/b", title: "드림 팝과 공간감" },
    { url: "https://example.invalid/c", title: "볼륨 대신 여백" },
  ],
  thumb_data_url: null,
};

/** 근거가 얇은 비평 — 카드 상단에 경고가 떠야 한다 (T58). */
export const thinCard: CulturalCard = {
  ...culturalCard,
  context_id: "01J8XQZN3M7P4RSTVWXYZ2",
  ingest_id: items[1]!.id,
  critique: "이 이미지가 속한 계보를 특정할 만한 근거를 충분히 찾지 못했다. 인접한 것만 겨우 짚는다.",
  lineage: ["뉴토포그래픽스"],
  grounded: false,
  sources: [{ url: "https://example.invalid/z", title: "근거 하나뿐" }],
};

export const config: Config = {
  api: { base_url: "https://api.openai.com/v1", timeout_secs: 120 },
  models: { vision: "gpt-x-vision", audio: "gpt-x-audio", text: "gpt-x", reflect: "gpt-x" },
  embed: { model: "text-embedding-3-small", dims: 256 },
  privacy: { show_boundary_on_first_run: true, boundary_acknowledged: true },
  youtube: { api_key: "", download_enabled: false },
  search: { provider: "duckduckgo", api_key: "", max_results: 8 },
  thresholds: {
    reflect_trigger_ingests: 20, axis_delta_max: 0.15, agent_max_turns_reflect: 12,
    agent_max_turns_critique: 8, agent_timeout_critique_secs: 90, context_enabled: true,
    critique_concurrency: 2, audio_cap_seconds: 30, video_max_seconds: 30, video_fps: 1,
    video_max_frames: 30, image_max_edge_px: 1280, agent_max_searches_critique: 3,
    list_observations_max: 200,
  },
  local: { silhouette_max_samples: 500, thumb_max_edge_px: 256 },
  mcp: { enabled: true, max_recall_limit: 50, prose_max_chars: 200 },
} as Config;

/** 한 슬롯을 일부러 실패시킨다 — 실패 표시가 어떻게 보이는지 봐야 한다 (§9.9). */
export const doctorReport: DoctorReport = {
  api_key_set: true,
  models_available: ["gpt-x", "gpt-x-vision", "gpt-x-audio"],
  slots: [
    { slot: "vision", model: "gpt-x-vision", ok: true, error: null },
    { slot: "audio", model: "gpt-x-audio", ok: false, error: "이 모델은 input_audio 를 받지 않습니다" },
    { slot: "text", model: "gpt-x", ok: true, error: null },
    { slot: "reflect", model: "gpt-x", ok: true, error: null },
  ],
  embed_ok: true,
  embed_error: null,
  // 셋 다 없는 상태로 둔다 — 앱을 다른 기계로 옮겼을 때가 정확히 이 모습이고,
  // 진단 화면의 안내 문구(무엇이 멈추는가 · 어떻게 설치하는가)가 그때 필요하다 (§9.7).
  ffmpeg: null,
  ffprobe: null,
  ytdlp: null,
  git_ok: true,
  soul_md_ok: true,
  multi_image_ok: true,
} as DoctorReport;

export const proposal: ProposalView = {
  current_text:
    "인공물보다 방치된 것을 고른다. 채도가 아니라 습도로\n공간을 읽고, 사람이 방금 나간 자리에 반복해서 멈춘다.",
  proposed_text: SOUL_MD,
  current_profile_text:
    "인공물보다 방치된 것을 고른다. 채도가 아니라 습도로\n공간을 읽고, 사람이 방금 나간 자리에 반복해서 멈춘다.",
  proposed_profile_text:
    "인공물보다 방치된 것을 고른다. 채도가 아니라 습도로 공간을 읽는다.\n" +
    "사람이 방금 나간 자리에 반복해서 멈추고, 그 정적이 오래 남기를 기다린다.\n" +
    "최근에는 소리가 사라지는 속도에도 같은 방식으로 반응한다.",
  axis_delta: { grain: 0.04, space: -0.02 },
  cites: [items[0]!.id, items[1]!.id, items[2]!.id],
  rationale: "습기·잔향 관련 정정이 3회 반복됨",
} as ProposalView;

export const detail: ItemDetail = {
  item: items[0]!,
  origin: "file:///Users/x/사진/2026-03-02.jpg",
  sensory_prose: PROSE[0]!,
  sensory_reading: { verdict: "no", prose: "향수 아니고 오히려 좀 서늘한 거리감", divergence: 0.41 },
  context: culturalCard,
  // 문화 층 미응답 — 상세에서 그 자리에 답할 수 있어야 한다 (T70).
  cultural_reading: null,
  context_failed: false,
  can_recast: false,
} as ItemDetail;

/** 아카이브 패싯을 실제로 적용한다 — 필터가 붙었을 때의 레이아웃을 보려면 필요하다. */
export function queryItems(q: Partial<ArchiveQuery> = {}): ArchiveItem[] {
  let out = items;
  if (q.kinds?.length) out = out.filter((i) => q.kinds!.includes(i.kind));
  if (q.cells?.length) out = out.filter((i) => q.cells!.includes(i.cell ?? "incomplete"));
  if (q.qualities?.length) out = out.filter((i) => q.qualities!.includes(i.quality));
  if (q.months?.length) out = out.filter((i) => q.months!.includes(i.month));
  if (q.tags?.length) out = out.filter((i) => q.tags!.some((t) => i.tags.includes(t)));
  if (q.cluster != null) out = out.filter((i) => i.cluster === q.cluster);
  if (q.surprisal_min != null) out = out.filter((i) => i.surprisal >= q.surprisal_min!);
  if (q.surprisal_max != null) out = out.filter((i) => i.surprisal <= q.surprisal_max!);
  if (q.search) {
    const n = String(q.search).toLowerCase();
    out = out.filter(
      (i) => i.prose.toLowerCase().includes(n) || i.tags.some((t) => t.includes(n)),
    );
  }
  return out;
}
