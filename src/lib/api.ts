/**
 * Tauri 커맨드 래퍼 — `src-tauri/src/commands.rs` 의 **완전한** 미러다.
 *
 * 여기 없는 것은 프런트가 부를 수 없다. 파일 시스템 임의 접근도, API 키 읽기도 없다 (§2).
 *
 * ## 인자 이름
 * Tauri v2 는 `#[tauri::command]` 인자를 기본적으로 **camelCase 로 받아 snake_case 로 매핑**한다.
 * 따라서 `ingest_id` 는 `{ ingestId }` 로, `probe_models` 는 `{ probeModels }` 로 보낸다.
 * 커맨드 **이름** 자체는 Rust 함수명 그대로(snake_case)다.
 * 백엔드가 `#[tauri::command(rename_all = "snake_case")]` 를 붙이면 이 파일도 함께 바꿔야 한다.
 *
 * ## 오류
 * Rust `Result<T, String>` 의 `Err` 는 invoke 를 **reject** 시킨다.
 * 호출부에서 try/catch 하고 `errorText(e)` 로 사람이 읽을 문장을 얻는다.
 */

import { invoke } from "@tauri-apps/api/core";
import type {
  ArchiveItem,
  ArchiveQuery,
  Config,
  CulturalCard,
  Derived,
  DoctorReport,
  ItemDetail,
  Kind,
  Layer,
  ProposalView,
  SaveResult,
  SecretStatus,
  SensoryCard,
  SetupStatus,
  Verdict,
} from "./types";

/**
 * 편의 재수출 — 화면은 `lib/api` 하나만 import 해도 된다.
 * 타입의 정본은 `lib/types.ts` 다. 새 타입은 그쪽에 추가한다.
 */
export * from "./types";

/** reject 된 invoke 의 값을 사람이 읽을 한 줄로 만든다. */
export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  if (e && typeof e === "object" && "message" in e) {
    const m = (e as { message: unknown }).message;
    if (typeof m === "string") return m;
  }
  try {
    return JSON.stringify(e);
  } catch {
    return String(e);
  }
}

// ───────────────────────────────────────────── 설정 · 진단 (§9.9 · §D7)

/** 앱 시작 시 가장 먼저 부른다. `needs_boundary_notice` 면 §D7 고지가 먼저다. */
export function setupStatus(): Promise<SetupStatus> {
  return invoke<SetupStatus>("setup_status");
}

/** §D7 — 고지 확인. `context_enabled` 선택을 **함께** 보낸다. */
export function acknowledgeBoundary(contextEnabled: boolean): Promise<void> {
  return invoke<void>("acknowledge_boundary", { contextEnabled });
}

/** 키를 키체인에 저장한다. **읽는 커맨드는 없다** (§2). */
export function setSecret(account: string, value: string): Promise<void> {
  return invoke<void>("set_secret", { account, value });
}

/** `[계정, 설정되어 있는가]` 목록. 값은 오지 않는다. */
export function secretsStatus(): Promise<SecretStatus[]> {
  return invoke<SecretStatus[]>("secrets_status");
}

/** §9.9 — `probeModels` 가 true 면 슬롯별로 실제 호출을 날려 검증한다(느리다). */
export function doctor(probeModels: boolean): Promise<DoctorReport> {
  return invoke<DoctorReport>("doctor", { probeModels });
}

export function getConfig(): Promise<Config> {
  return invoke<Config>("get_config");
}

export function setConfig(config: Config): Promise<void> {
  return invoke<void>("set_config", { config });
}

// ───────────────────────────────────────────────────── 투입 (§13 화면 1)

/** 드래그앤드롭은 **파일 경로만** 준다. URL 드롭 경로는 만들지 않는다 (§13 화면 1). */
export function ingestFiles(paths: string[]): Promise<SensoryCard[]> {
  return invoke<SensoryCard[]>("ingest_files", { paths });
}

/** 클립보드 투입(`Cmd/Ctrl+Shift+V`). URL이면 YouTube만 받는다 (§9.1 · T11c). */
export function ingestClipboard(text: string): Promise<SensoryCard> {
  return invoke<SensoryCard>("ingest_clipboard", { text });
}

/**
 * §9.10 — 화면 1에 **조용히** 표시할 문화 글귀 대기 건수.
 * 건별 알림을 띄우지 않는다 (T51e).
 */
export function critiquePendingCount(): Promise<number> {
  return invoke<number>("critique_pending_count");
}

// ────────────────────────────────────────── ○/× 응답 (§13 화면 2 · 2.1)

/**
 * ○/× 를 기록하고 새 `reading` 관측의 ID를 돌려준다.
 *
 * - `verdict` 는 `"yes"` | `"no"` 뿐이다. 중간값을 만들지 말 것 (T59).
 * - `verdict: "yes"` 면 `prose` 는 반드시 `null` 이다 (§6.3 · T7).
 * - `layer: "cultural"` 의 `target` 은 `ingest_id` 가 아니라 **`context_id`** 다 (§12.6).
 */
export function recordReading(
  target: string,
  layer: Layer,
  verdict: Verdict,
  prose: string | null,
): Promise<string> {
  return invoke<string>("record_reading", { target, layer, verdict, prose });
}

/** §13 화면 2.1 — 미응답 문화 카드. **앱 재시작 후에도 남는다** (T53). */
export function pendingCulturalCards(): Promise<CulturalCard[]> {
  return invoke<CulturalCard[]>("pending_cultural_cards");
}

/** §9.3 — kind 뒤집기. `recastPreview` 로 버려질 응답 수를 먼저 확인시킨다. */
export function recastKind(ingestId: string, kind: Kind): Promise<SensoryCard> {
  return invoke<SensoryCard>("recast_kind", { ingestId, kind });
}

/** §9.3 — 뒤집으면 버려질 응답 수. */
export function recastPreview(ingestId: string): Promise<number> {
  return invoke<number>("recast_preview", { ingestId });
}

/** §9.10 — 문화 글귀 재시도. 검색 결과는 시간이 지나면 달라진다. */
export function redoContext(ingestId: string): Promise<void> {
  return invoke<void>("redo_context", { ingestId });
}

// ─────────────────────────────────────────────────── SOUL.md (§13 화면 3)

/** 로컬 표시용이므로 `soul:human` 을 포함한 원문 그대로 온다 (§D4). */
export function readSoulMd(): Promise<string> {
  return invoke<string>("read_soul_md");
}

/** §8.3 · §8.4 — 저장. `gen_blocks_modified` 가 비어 있지 않으면 경고를 보여준다. */
export function saveSoulMd(text: string): Promise<SaveResult> {
  return invoke<SaveResult>("save_soul_md", { text });
}

// ─────────────────────────────────────────────────── 성찰 (§13 화면 4)

/** 제안이 없으면 `null`. `force` 는 §11.2 트리거 조건을 무시한다. */
export function reflect(force: boolean): Promise<ProposalView | null> {
  return invoke<ProposalView | null>("reflect", { force });
}

/** 수정 후 승인이면 수정된 텍스트를, 그대로 승인이면 `null` 을 보낸다. */
export function approveProposal(modifiedText: string | null): Promise<string> {
  return invoke<string>("approve_proposal", { modifiedText });
}

export function rejectProposal(): Promise<void> {
  return invoke<void>("reject_proposal");
}

// ────────────────────────────────────────────── 대시보드 (§13 화면 5)

/** **읽기만.** 프런트에서 계산하지 않는다 (§20.7). 첫 렌더 500ms 예산 (T39). */
export function dashboard(): Promise<Derived> {
  return invoke<Derived>("dashboard");
}

// ───────────────────────────────────────────── 아카이브 (§13 화면 6)

/** **어떤 필터도 API를 호출하지 않는다** (T68). `emptyArchiveQuery()` 로 시작해라. */
export function archiveQuery(query: ArchiveQuery): Promise<ArchiveItem[]> {
  return invoke<ArchiveItem[]>("archive_query", { query });
}

/** 캐시된 벡터끼리 코사인. **API 호출 없음** (T33 · T68). */
export function archiveNeighbors(id: string, n: number): Promise<ArchiveItem[]> {
  return invoke<ArchiveItem[]>("archive_neighbors", { id, n });
}

export function archiveDetail(id: string): Promise<ItemDetail> {
  return invoke<ItemDetail>("archive_detail", { id });
}

// ─────────────────────────────────────────────────── 기타 (§14 · §19.7)

/** §19 — 로컬 MCP 서버 설정 JSON. 사용자가 복사해 붙여넣는다. */
export function mcpConfigJson(): Promise<string> {
  return invoke<string>("mcp_config_json");
}

/** §19.8 — 축소 경로. `SOUL.md` 를 프롬프트 문자열로 내보낸다. */
export function exportPrompt(): Promise<string> {
  return invoke<string>("export_prompt");
}

/** `runs/` 전체 삭제. 지운 파일 수를 돌려준다. */
export function tracePurge(): Promise<number> {
  return invoke<number>("trace_purge");
}

/** §14 — `fromScratch` 는 `derived.sqlite` 를 삭제 후 전량 재구축한다. */
export function rebuild(fromScratch: boolean, offline: boolean): Promise<string> {
  return invoke<string>("rebuild", { fromScratch, offline });
}
