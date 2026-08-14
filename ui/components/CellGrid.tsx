/**
 * 2×2 셀 분포 — **직접 쓴 인라인 SVG** (§2 · §20.8).
 *
 * §13 화면 5 패널 2: 네 칸에 각 셀의 개수와 30일 추이 화살표.
 * `other_reason` 칸을 강조한다 — 이 시스템이 찾으려는 것이다 (§12.6).
 *
 * **실수 지표(divergence)는 여기에 싣지 않는다** (§12.6 — 화면에는 2×2 격자만).
 *
 * 셀 판정은 커맨드가 한다 (§12.6의 2단 조인). 이 컴포넌트는 개수를 놓기만 한다.
 */

import { CELL_MEANINGS, CELLS, EM_DASH } from "../lib/types";
import type { CellCounts, CellName } from "../lib/types";

/** 2×2 셀 식별자 (§12.6). `lib/types.ts` 의 `CellName` 과 같은 값이다. */
export type CellKey = CellName;

export interface CellGridProps {
  /** §12.6 — 전체 기간 셀 개수. */
  cells: CellCounts;
  /** §12.6 — 최근 30일 셀 개수. 추이 화살표에 쓴다. */
  recent30: CellCounts;
  /** supersede되지 않은 ingest 수 (§R9). 각주의 "미완성" 계산에만 쓴다. */
  observationCount?: number;
  onSelect?: (cell: CellName) => void;
}

/** §12.6 표의 행 순서 그대로. 2×2 배치가 이 순서에 걸려 있다. */
const ORDER = CELLS;

const W = 348;
const CELL_W = 122;
const CELL_H = 90;
const GRID_X = 96;
const GRID_Y = 46;
const H = GRID_Y + CELL_H * 2 + 40;

function total(c: CellCounts): number {
  return c.read + c.other_reason + c.wrong_words + c.unread;
}

/**
 * 30일 추이 — 최근 30일의 **비중**을 전체 기간의 비중과 견준다.
 * 개수를 직접 견주면 최근 30일이 전체의 부분집합이라 늘 작게 나온다.
 * 표본이 없으면 화살표를 그리지 않고 `—`를 쓴다 (§R10).
 */
function trend(cell: CellName, cells: CellCounts, recent: CellCounts): { mark: string; cls: string; title: string } {
  const tAll = total(cells);
  const tRec = total(recent);
  if (tAll === 0 || tRec === 0) {
    return { mark: EM_DASH, cls: "is-flat", title: "최근 30일 표본 없음" };
  }
  const shareAll = cells[cell] / tAll;
  const shareRec = recent[cell] / tRec;
  const d = shareRec - shareAll;
  const pct = `최근 30일 ${recent[cell]}건 · 비중 ${(shareRec * 100).toFixed(0)}% (전체 ${(shareAll * 100).toFixed(0)}%)`;
  if (d > 0.02) return { mark: "▲", cls: "is-up", title: pct };
  if (d < -0.02) return { mark: "▼", cls: "is-down", title: pct };
  return { mark: "·", cls: "is-flat", title: pct };
}

export default function CellGrid({ cells, recent30, observationCount, onSelect }: CellGridProps) {
  const done = total(cells);

  if (done === 0) {
    return (
      <div className="chart-empty-box">
        <p>아직 두 층 모두에 답한 항목이 없습니다.</p>
        <p className="chart-empty-sub">감각 글귀와 문화 글귀 양쪽에 답하면 이 칸이 채워집니다.</p>
      </div>
    );
  }

  const pending = observationCount !== undefined ? Math.max(0, observationCount - done) : null;

  return (
    <div className="chart-cellgrid">
      <svg
        className="chart-svg"
        viewBox={`0 0 ${W} ${H}`}
        style={{ width: "100%", height: "auto" }}
        role="img"
        aria-label="2×2 셀 분포"
      >
        {/* 열 머리 — 문화 층 */}
        <text className="cg-head" x={GRID_X + CELL_W / 2} y={GRID_Y - 22} textAnchor="middle">
          그래서 좋다
        </text>
        <text className="cg-head" x={GRID_X + CELL_W + CELL_W / 2} y={GRID_Y - 22} textAnchor="middle">
          그것 때문은 아니다
        </text>
        <text className="cg-legend" x={GRID_X + CELL_W} y={GRID_Y - 36} textAnchor="middle">
          문화 글귀
        </text>

        {/* 행 머리 — 감각 층 */}
        <text className="cg-head" x={GRID_X - 10} y={GRID_Y + CELL_H / 2 + 4} textAnchor="end">
          그렇다
        </text>
        <text className="cg-head" x={GRID_X - 10} y={GRID_Y + CELL_H + CELL_H / 2 + 4} textAnchor="end">
          아니다
        </text>
        <text
          className="cg-legend"
          transform={`translate(14 ${GRID_Y + CELL_H}) rotate(-90)`}
          textAnchor="middle"
        >
          감각 글귀
        </text>

        {ORDER.map((key, i) => {
          const col = i % 2;
          const row = Math.floor(i / 2);
          const x = GRID_X + col * CELL_W;
          const y = GRID_Y + row * CELL_H;
          const t = trend(key, cells, recent30);
          const emphasis = key === "other_reason";
          const n = cells[key];
          const share = done > 0 ? (n / done) * 100 : 0;
          return (
            <g
              key={key}
              className={`cg-cell cg-${key}${emphasis ? " is-emphasis" : ""}${onSelect ? " is-clickable" : ""}`}
              onClick={onSelect ? () => onSelect(key) : undefined}
              role={onSelect ? "button" : undefined}
              tabIndex={onSelect ? 0 : undefined}
              onKeyDown={
                onSelect
                  ? (e) => {
                      if (e.key === "Enter" || e.key === " ") onSelect(key);
                    }
                  : undefined
              }
            >
              <rect className="cg-box" x={x + 3} y={y + 3} width={CELL_W - 6} height={CELL_H - 6} rx={4} />
              <text className="cg-key" x={x + 13} y={y + 21}>
                {key}
              </text>
              <text className="cg-count" x={x + 13} y={y + 51}>
                {n}
              </text>
              <text className={`cg-trend ${t.cls}`} x={x + CELL_W - 15} y={y + 51} textAnchor="end">
                {t.mark}
              </text>
              <text className="cg-share" x={x + 13} y={y + 70}>
                {share.toFixed(0)}%
              </text>
              <title>{`${key} ${EM_DASH} ${CELL_MEANINGS[key]}\n${n}건\n${t.title}`}</title>
            </g>
          );
        })}

        <text className="cg-foot" x={GRID_X} y={H - 14}>
          {pending === null
            ? `두 층 응답 완료 ${done}건`
            : `두 층 응답 완료 ${done}건 · 미완성 ${pending}건`}
        </text>
      </svg>
    </div>
  );
}
