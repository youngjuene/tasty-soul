/**
 * `@tauri-apps/api/core` 의 `invoke` 를 대신한다.
 *
 * `npm run design` 은 이 파일을 별칭으로 끼워 넣어 **Rust · Tauri · API 키 없이**
 * 화면을 띄운다. 화면 코드는 이 파일의 존재를 모른다 — 앱 빌드에는 들어가지 않는다.
 *
 * 새 커맨드를 `src-tauri/src/commands.rs` 에 추가했는데 미리보기에서
 * "미리보기 목에 없는 커맨드" 가 뜨면, 여기 `handlers` 에 한 줄 추가하면 된다.
 */

import * as F from "./fixtures";

const handlers: Record<string, (a: Record<string, unknown>) => unknown> = {
  // ── 설정 · 진단
  setup_status: () => ({
    first_run: false,
    needs_boundary_notice: false,
    api_key_set: true,
    models_unset: false,
    context_enabled: true,
  }),
  secrets_status: () => [["openai", true], ["search", false], ["youtube", false]],
  get_config: () => F.config,
  set_config: () => null,
  doctor: () => F.doctorReport,
  acknowledge_boundary: () => null,
  set_secret: () => null,
  mcp_config_json: () =>
    JSON.stringify({ mcpServers: { soul: { command: "soul", args: ["mcp"] } } }, null, 2),
  export_prompt: () => "# SOUL\n\n기준 2026-08-13 · 관측 342 · 최초 2026-03-02\n",
  trace_purge: () => 12,
  rebuild: () => "관측 901건 재생 · SOUL.md 재작성 · 커밋 1개",

  // ── 투입 · 카드
  ingest_files: () => [F.sensoryCard, F.guessCard],
  ingest_clipboard: () => F.guessCard,
  critique_pending_count: () => 3,
  pending_cultural_cards: () => [F.culturalCard, F.thinCard],
  record_reading: () => "01J8XQZL3M7P4RSTVWXYZ3",
  recast_preview: () => 2,
  recast_kind: () => F.sensoryCard,
  redo_context: () => null,

  // ── SOUL.md · 성찰
  read_soul_md: () => F.SOUL_MD,
  save_soul_md: () => ({ profile_edits: 1, commits: 2, gen_blocks_modified: [] }),
  reflect: () => F.proposal,
  approve_proposal: () => "01J8XQZM3M7P4RSTVWXYZ2",
  reject_proposal: () => null,

  // ── 대시보드 · 아카이브
  dashboard: () => F.derived,
  archive_query: (a) => F.queryItems((a?.query ?? {}) as never),
  archive_neighbors: () => F.items.slice(0, 5),
  archive_detail: () => F.detail,
};

/** 실제 IPC 처럼 약간 늦게 온다 — 로딩 상태가 실제로 보이도록. */
export async function invoke(cmd: string, args?: Record<string, unknown>): Promise<unknown> {
  const h = handlers[cmd];
  if (!h) throw new Error(`미리보기 목에 없는 커맨드: ${cmd} (ui/preview/mock.ts 에 추가하세요)`);
  await new Promise((r) => setTimeout(r, 60));
  return h(args ?? {});
}

// ── 플러그인들이 같은 모듈에서 가져오는 것들. 목에서는 무해한 스텁이다.
export class Channel<T = unknown> {
  onmessage: ((m: T) => void) | null = null;
  toJSON() { return "__CHANNEL__"; }
}
export class PluginListener {
  async unregister() {}
}
export async function addPluginListener() { return new PluginListener(); }
export function transformCallback(cb: (r: unknown) => void) { void cb; return 0; }
export function convertFileSrc(p: string) { return p; }
export async function checkPermissions() { return "granted"; }
export async function requestPermissions() { return "granted"; }
export function isTauri() { return false; }
