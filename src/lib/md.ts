/**
 * 아주 작은 마크다운 파서 (§13 화면 3).
 *
 * 라이브러리를 쓰지 않는다. `SOUL.md`의 문법은 §8.2 템플릿이 정한 것 —
 * 제목 · 표 · 목록 · 문단뿐이다. 인라인 강조 문법은 쓰지 않으므로 파싱하지 않는다.
 *
 * 파싱은 §8.3 규칙 1과 같은 **줄 단위 상태 기계**다. 정규식으로 문서 전체를 훑지 않는다.
 * 마커 주석(`<!-- soul:... -->`)은 렌더에서 숨긴다.
 *
 * 이 모듈은 순수 함수만 둔다. 판단·계산은 Rust에 있다 (§2) — 여기서 하는 일은
 * 이미 만들어진 문자열을 화면에 놓기 위해 쪼개는 것뿐이다.
 */

export type MdNode =
  | { kind: "heading"; level: number; text: string }
  | { kind: "paragraph"; text: string }
  | { kind: "list"; ordered: boolean; items: string[] }
  | { kind: "table"; head: string[]; rows: string[][] };

export type SoulBlockKind = "gen" | "neg" | "human";

export interface SoulBlock {
  kind: SoulBlockKind;
  /** `id=` 속성. `soul:human`에는 없다 */
  id: string | null;
  attrs: Record<string, string>;
  /** 여는 마커 줄 (0-based) */
  markerStart: number;
  /** 닫는 마커 줄 (0-based). 짝이 맞지 않으면 `-1` */
  markerEnd: number;
}

/** §8.3 규칙 2 — 마커 줄은 앞뒤 공백만 허용한다. */
const OPEN_RE = /^<!--\s*soul:(gen|neg|human)([^>]*?)-->$/;
const CLOSE_RE = /^<!--\s*\/soul:(gen|neg|human)\s*-->$/;
const ATTR_RE = /([A-Za-z_][\w-]*)=([^\s]+)/g;

export function isMarkerLine(line: string): boolean {
  const t = line.trim();
  return OPEN_RE.test(t) || CLOSE_RE.test(t);
}

function parseAttrs(raw: string): Record<string, string> {
  const out: Record<string, string> = {};
  ATTR_RE.lastIndex = 0;
  let m: RegExpExecArray | null;
  while ((m = ATTR_RE.exec(raw)) !== null) out[m[1]] = m[2];
  return out;
}

export function toLines(src: string): string[] {
  return src.replace(/\r\n/g, "\n").split("\n");
}

/**
 * 마커 블록의 줄 범위를 찾는다. 편집 모드에서 `soul:gen` 영역을 회색 처리하는 데 쓴다.
 * 짝이 맞지 않아도 **여기서는 실패시키지 않는다** — 저장 시 백엔드가 판단한다 (§8.3 규칙 6).
 */
export function scanSoulBlocks(src: string): SoulBlock[] {
  const lines = toLines(src);
  const blocks: SoulBlock[] = [];
  let open: SoulBlock | null = null;

  for (let i = 0; i < lines.length; i++) {
    const t = lines[i].trim();

    const close = CLOSE_RE.exec(t);
    if (close) {
      if (open && open.kind === close[1]) {
        open.markerEnd = i;
        blocks.push(open);
        open = null;
      }
      continue;
    }

    const openM = OPEN_RE.exec(t);
    if (openM) {
      if (open) blocks.push(open); // 닫히지 않은 채 다음 블록이 열렸다
      const attrs = parseAttrs(openM[2]);
      open = {
        kind: openM[1] as SoulBlockKind,
        id: attrs.id ?? null,
        attrs,
        markerStart: i,
        markerEnd: -1,
      };
    }
  }
  if (open) blocks.push(open);
  return blocks;
}

function splitRow(line: string): string[] {
  let s = line.trim();
  if (s.startsWith("|")) s = s.slice(1);
  if (s.endsWith("|")) s = s.slice(0, -1);
  return s.split("|").map((c) => c.trim());
}

function isTableSeparator(line: string): boolean {
  const t = line.trim();
  if (!t.includes("-")) return false;
  if (!t.includes("|")) return false;
  const cells = splitRow(t);
  return cells.length > 0 && cells.every((c) => /^:?-+:?$/.test(c));
}

function isTableRow(line: string): boolean {
  return line.trim().includes("|");
}

const HEADING_RE = /^(#{1,6})\s+(.*)$/;
const UL_RE = /^\s*[-*+]\s+(.*)$/;
const OL_RE = /^\s*\d+[.)]\s+(.*)$/;

/**
 * 마커 주석을 감춘 뒤 블록 단위로 쪼갠다.
 * 마커 줄은 **빈 줄로 치환한다** — 지우면 앞뒤 문단이 하나로 붙는다.
 */
export function parseMarkdown(src: string): MdNode[] {
  const lines = toLines(src).map((l) => (isMarkerLine(l) ? "" : l));
  const out: MdNode[] = [];
  let i = 0;

  while (i < lines.length) {
    const line = lines[i];

    if (line.trim() === "") {
      i++;
      continue;
    }

    const h = HEADING_RE.exec(line);
    if (h) {
      out.push({ kind: "heading", level: h[1].length, text: h[2].trim() });
      i++;
      continue;
    }

    // 표 — 머리 줄 + 구분 줄이 붙어 있어야 한다
    if (isTableRow(line) && i + 1 < lines.length && isTableSeparator(lines[i + 1])) {
      const head = splitRow(line);
      const rows: string[][] = [];
      i += 2;
      while (i < lines.length && lines[i].trim() !== "" && isTableRow(lines[i])) {
        rows.push(splitRow(lines[i]));
        i++;
      }
      out.push({ kind: "table", head, rows });
      continue;
    }

    const ulM = UL_RE.exec(line);
    const olM = OL_RE.exec(line);
    if (ulM || olM) {
      const ordered = !ulM;
      const items: string[] = [];
      while (i < lines.length) {
        const m = ordered ? OL_RE.exec(lines[i]) : UL_RE.exec(lines[i]);
        if (!m) break;
        items.push(m[1].trim());
        i++;
      }
      out.push({ kind: "list", ordered, items });
      continue;
    }

    // 문단 — 빈 줄이나 다른 블록이 나올 때까지 모은다.
    // 줄바꿈은 **보존한다** (`.doc-md-p`가 `white-space: pre-line`). 헤더 블록처럼
    // 줄 나눔이 뜻을 가지는 곳이 있고, 사용자는 자기 파일을 보는 것이기 때문이다.
    const buf: string[] = [];
    while (i < lines.length) {
      const l = lines[i];
      if (l.trim() === "") break;
      if (HEADING_RE.test(l)) break;
      if (UL_RE.test(l) || OL_RE.test(l)) break;
      if (isTableRow(l) && i + 1 < lines.length && isTableSeparator(lines[i + 1])) break;
      buf.push(l.trim());
      i++;
    }
    if (buf.length > 0) out.push({ kind: "paragraph", text: buf.join("\n") });
  }

  return out;
}
