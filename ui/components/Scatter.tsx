/**
 * 산점도 — **직접 쓴 인라인 SVG** (§2 · §20.8). 차트 라이브러리를 쓰지 않는다.
 *
 * 두 곳에서 쓴다.
 *  - 대시보드 패널 1 (§13 화면 5): `(drift, crystal)` 연결 산점도. `connect` + `showLabels`.
 *  - 아카이브 (§13 화면 6): 항목 산점도. `interactive` + 밀도 전환.
 *
 * 밀도 처리 (§13 화면 6 · T67):
 *   보이는 항목 ≤ tileThreshold(기본 200) → 썸네일 타일
 *   초과                                  → 단색 점. 확대하면 타일로 전환
 *
 * 성능 (T39 · T67): 5,000건에서 첫 렌더 500ms 예산.
 *   - 점 모드는 색 그룹당 `<path>` **한 개**로 그린다. DOM 노드가 항목 수에 비례하지 않는다.
 *   - 타일은 **화면에 들어온 것만** 만든다.
 *   - 히트 테스트는 DOM이 아니라 좌표 스캔이다.
 *
 * 이 컴포넌트는 좌표를 화면에 놓기만 한다. 좌표 자체는 커맨드가 준 값이다 (§2).
 */

import { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { PointerEvent as ReactPointerEvent, RefObject } from "react";

export interface ScatterPoint {
  id: string;
  x: number;
  y: number;
  /** 점 옆에 쓰는 짧은 라벨. 패널 1의 `2026-06`. */
  label?: string;
  /** 타일 썸네일. `null`이면 `text`를 쓴다 (T70c). */
  thumb?: string | null;
  /** 썸네일이 없을 때 타일에 렌더할 글. 호출자가 앞 40자를 잘라 넘긴다 (T70c). */
  text?: string | null;
  /** 색 그룹 키. 점 모드에서 그룹당 path 하나로 묶인다. */
  group?: string;
  /** 강조 (패널 1의 현재 달). */
  emphasis?: boolean;
}

export type ScatterMode = "tile" | "dot";

export interface ScatterProps {
  points: ScatterPoint[];
  xLabel: string;
  yLabel: string;
  height?: number;
  /** 배열 순서대로 선을 잇는다. 패널 1은 시간순으로 정렬해 넘긴다. */
  connect?: boolean;
  showLabels?: boolean;
  /** 확대·이동 허용. */
  interactive?: boolean;
  /** group → 색. 없으면 기본색. */
  colorOf?: (group: string | undefined) => string;
  selectedId?: string | null;
  onSelect?: (id: string) => void;
  /** 타일/점 전환 임계 (§13 화면 6). */
  tileThreshold?: number;
  onVisibleChange?: (visible: number, mode: ScatterMode) => void;
  emptyMessage?: string;
  /** 축 범위 고정. 없으면 데이터 범위에서 잡는다. */
  xDomain?: [number, number];
  yDomain?: [number, number];
  /** 타일 한 변(px). */
  tileSize?: number;
}

const M = { top: 14, right: 18, bottom: 36, left: 50 };
const MIN_K = 1;
const MAX_K = 40;
const HIT_RADIUS = 14;

// ─────────────────────────────────────────────────────────────── 눈금

/** 1·2·5 × 10^n 단위의 눈금. 라이브러리 없이 쓴다. */
export function niceTicks(a: number, b: number, count: number): number[] {
  if (!isFinite(a) || !isFinite(b) || a === b) return [a];
  const span = b - a;
  const raw = span / Math.max(1, count);
  const mag = Math.pow(10, Math.floor(Math.log10(raw)));
  const norm = raw / mag;
  const step = (norm >= 5 ? 5 : norm >= 2 ? 2 : 1) * mag;
  const out: number[] = [];
  const start = Math.ceil(a / step) * step;
  for (let v = start; v <= b + step * 1e-6 && out.length < 40; v += step) {
    out.push(Math.abs(v) < step * 1e-6 ? 0 : v);
  }
  // 부동소수 오차로 눈금이 촘촘해질 때가 있다. 겹쳐 읽히느니 솎아 낸다.
  if (out.length > count * 2 + 1) {
    const stride = Math.ceil(out.length / (count + 1));
    return out.filter((_, i) => i % stride === 0);
  }
  return out;
}

function fmtTick(v: number): string {
  // `-0` 을 내지 않는다. `(-0.0004).toFixed(2)` → `"-0.00"` → 뒤 0 제거 → `"-0"` 이라
  // 축 눈금에 `-0` 이 줄줄이 찍혔다. 0이면 부호를 뗀다.
  const r0 = Math.round(v * 1000) / 1000;
  const r = r0 === 0 ? 0 : r0;
  if (Number.isInteger(r)) return String(r);
  return r.toFixed(Math.abs(r) < 1 ? 2 : 1).replace(/0+$/, "").replace(/\.$/, "");
}

/** 타일 글줄 나누기. 한국어라 글자수 기준으로 자른다. */
export function wrapChars(s: string, per: number, maxLines: number): string[] {
  const t = s.replace(/\s+/g, " ").trim();
  const out: string[] = [];
  let i = 0;
  while (i < t.length && out.length < maxLines) {
    if (t[i] === " ") {
      i += 1; // 줄 첫머리의 공백은 버린다 — 왼쪽 끝이 들쭉날쭉해 보인다
      continue;
    }
    out.push(t.slice(i, i + per));
    i += per;
  }
  return out;
}

/** React의 `useId`는 콜론을 담고 있어 SVG `url(#…)`에 그대로 쓸 수 없다. */
function useClipId(): string {
  return "clip" + useId().replace(/[^a-zA-Z0-9]/g, "");
}

// ─────────────────────────────────────────────────────────── 크기 관측

function useBoxWidth(ref: RefObject<HTMLDivElement>): number {
  const [w, setW] = useState(0);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    setW(el.clientWidth);
    const ro = new ResizeObserver((entries) => {
      const r = entries[0]?.contentRect;
      if (r) setW(Math.round(r.width));
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref]);
  return w;
}

// ─────────────────────────────────────────────────────────────── 본체

export default function Scatter(props: ScatterProps) {
  const {
    points,
    xLabel,
    yLabel,
    height = 380,
    connect = false,
    showLabels = false,
    interactive = false,
    colorOf,
    selectedId = null,
    onSelect,
    tileThreshold = 200,
    onVisibleChange,
    emptyMessage = "표시할 항목이 없습니다",
    xDomain,
    yDomain,
    tileSize = 60,
  } = props;

  const boxRef = useRef<HTMLDivElement>(null);
  const svgRef = useRef<SVGSVGElement>(null);
  const width = useBoxWidth(boxRef);
  const [view, setView] = useState({ k: 1, tx: 0, ty: 0 });
  const [hoverId, setHoverId] = useState<string | null>(null);
  const dragRef = useRef<{ id: number; x: number; y: number; moved: boolean } | null>(null);

  const innerW = Math.max(10, width - M.left - M.right);
  const innerH = Math.max(10, height - M.top - M.bottom);

  // 데이터 범위 — 좌표를 화면에 놓기 위한 스케일일 뿐, 파생값 계산이 아니다.
  const [x0, x1, y0, y1] = useMemo(() => {
    if (xDomain && yDomain) return [xDomain[0], xDomain[1], yDomain[0], yDomain[1]];
    let ax = Infinity;
    let bx = -Infinity;
    let ay = Infinity;
    let by = -Infinity;
    for (const p of points) {
      if (p.x < ax) ax = p.x;
      if (p.x > bx) bx = p.x;
      if (p.y < ay) ay = p.y;
      if (p.y > by) by = p.y;
    }
    if (!isFinite(ax)) {
      ax = 0;
      bx = 1;
      ay = 0;
      by = 1;
    }
    const padX = (bx - ax) * 0.08 || 0.05;
    const padY = (by - ay) * 0.08 || 0.05;
    const dx: [number, number] = xDomain ?? [ax - padX, bx + padX];
    const dy: [number, number] = yDomain ?? [ay - padY, by + padY];
    return [dx[0], dx[1], dy[0], dy[1]];
  }, [points, xDomain, yDomain]);

  const spanX = x1 - x0 || 1;
  const spanY = y1 - y0 || 1;

  const baseX = useCallback((v: number) => M.left + ((v - x0) / spanX) * innerW, [x0, spanX, innerW]);
  const baseY = useCallback((v: number) => M.top + innerH - ((v - y0) / spanY) * innerH, [y0, spanY, innerH]);

  const sx = useCallback((v: number) => baseX(v) * view.k + view.tx, [baseX, view]);
  const sy = useCallback((v: number) => baseY(v) * view.k + view.ty, [baseY, view]);

  /** 화면 좌표 → 데이터 좌표 (눈금 범위 계산용). */
  const invX = useCallback(
    (s: number) => (((s - view.tx) / view.k - M.left) / innerW) * spanX + x0,
    [view, innerW, spanX, x0],
  );
  const invY = useCallback(
    (s: number) => ((M.top + innerH - (s - view.ty) / view.k) / innerH) * spanY + y0,
    [view, innerH, spanY, y0],
  );

  // 화면에 들어온 것만 (§13 화면 6 — 썸네일은 보이는 것만 읽는다)
  const placed = useMemo(() => {
    const L = M.left;
    const R = M.left + innerW;
    const T = M.top;
    const B = M.top + innerH;
    const pad = tileSize;
    const out: { p: ScatterPoint; cx: number; cy: number }[] = [];
    for (const p of points) {
      const cx = sx(p.x);
      const cy = sy(p.y);
      if (cx < L - pad || cx > R + pad || cy < T - pad || cy > B + pad) continue;
      out.push({ p, cx, cy });
    }
    return out;
  }, [points, sx, sy, innerW, innerH, tileSize]);

  const mode: ScatterMode = placed.length > tileThreshold ? "dot" : "tile";

  useEffect(() => {
    onVisibleChange?.(placed.length, mode);
  }, [placed.length, mode, onVisibleChange]);

  // ── 상호작용

  const zoomAt = useCallback((px: number, py: number, factor: number) => {
    setView((v) => {
      const k = Math.min(MAX_K, Math.max(MIN_K, v.k * factor));
      if (k === v.k) return v;
      const r = k / v.k;
      return { k, tx: px - r * (px - v.tx), ty: py - r * (py - v.ty) };
    });
  }, []);

  // React는 wheel을 passive로 붙이므로 `preventDefault`가 먹지 않는다.
  // 확대하는 동안 화면이 같이 스크롤되지 않도록 직접 건다.
  useEffect(() => {
    const el = svgRef.current;
    if (!el || !interactive) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const rect = el.getBoundingClientRect();
      zoomAt(e.clientX - rect.left, e.clientY - rect.top, Math.exp(-e.deltaY * 0.0015));
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [interactive, zoomAt, width]);

  const nearest = useCallback(
    (px: number, py: number): string | null => {
      let best: string | null = null;
      let bestD = HIT_RADIUS * HIT_RADIUS;
      for (const { p, cx, cy } of placed) {
        const dx = cx - px;
        const dy = cy - py;
        const d = dx * dx + dy * dy;
        if (d < bestD) {
          bestD = d;
          best = p.id;
        }
      }
      return best;
    },
    [placed],
  );

  const onPointerDown = useCallback(
    (e: ReactPointerEvent<SVGSVGElement>) => {
      if (!interactive) return;
      e.currentTarget.setPointerCapture(e.pointerId);
      dragRef.current = { id: e.pointerId, x: e.clientX, y: e.clientY, moved: false };
    },
    [interactive],
  );

  const onPointerMove = useCallback(
    (e: ReactPointerEvent<SVGSVGElement>) => {
      const rect = e.currentTarget.getBoundingClientRect();
      const d = dragRef.current;
      if (d && d.id === e.pointerId) {
        const dx = e.clientX - d.x;
        const dy = e.clientY - d.y;
        if (!d.moved && Math.abs(dx) + Math.abs(dy) < 3) return;
        d.moved = true;
        d.x = e.clientX;
        d.y = e.clientY;
        setView((v) => ({ k: v.k, tx: v.tx + dx, ty: v.ty + dy }));
        return;
      }
      if (onSelect || showLabels) setHoverId(nearest(e.clientX - rect.left, e.clientY - rect.top));
    },
    [nearest, onSelect, showLabels],
  );

  const onPointerUp = useCallback(
    (e: ReactPointerEvent<SVGSVGElement>) => {
      const d = dragRef.current;
      dragRef.current = null;
      if (!onSelect) return;
      if (d && d.moved) return;
      const rect = e.currentTarget.getBoundingClientRect();
      const hit = nearest(e.clientX - rect.left, e.clientY - rect.top);
      if (hit) onSelect(hit);
    },
    [nearest, onSelect],
  );

  const reset = useCallback(() => setView({ k: 1, tx: 0, ty: 0 }), []);

  // ── 그리기

  const xTicks = useMemo(
    () => niceTicks(invX(M.left), invX(M.left + innerW), 5),
    [invX, innerW],
  );
  const yTicks = useMemo(
    () => niceTicks(invY(M.top + innerH), invY(M.top), 4),
    [invY, innerH],
  );

  /** 점 모드 — 색 그룹당 path 하나. DOM 노드가 항목 수에 비례하지 않는다. */
  const dotPaths = useMemo(() => {
    if (mode !== "dot") return [];
    const byGroup = new Map<string, string[]>();
    for (const { p, cx, cy } of placed) {
      const g = p.group ?? "";
      let arr = byGroup.get(g);
      if (!arr) {
        arr = [];
        byGroup.set(g, arr);
      }
      arr.push(`M${cx.toFixed(1)} ${cy.toFixed(1)}h.01`);
    }
    return [...byGroup.entries()].map(([g, segs]) => ({ group: g, d: segs.join("") }));
  }, [mode, placed]);

  const linePath = useMemo(() => {
    if (!connect || points.length < 2) return "";
    return points.map((p, i) => `${i === 0 ? "M" : "L"}${sx(p.x).toFixed(1)} ${sy(p.y).toFixed(1)}`).join("");
  }, [connect, points, sx, sy]);

  const color = useCallback((g: string | undefined) => colorOf?.(g) ?? "var(--ch-mark)", [colorOf]);
  const half = tileSize / 2;
  const clipId = useClipId();

  const empty = points.length === 0;

  return (
    <div className="chart-scatter" ref={boxRef}>
      {interactive && (
        <div className="chart-zoom">
          <button type="button" onClick={() => zoomAt(M.left + innerW / 2, M.top + innerH / 2, 1.4)} aria-label="확대">
            +
          </button>
          <button type="button" onClick={() => zoomAt(M.left + innerW / 2, M.top + innerH / 2, 1 / 1.4)} aria-label="축소">
            −
          </button>
          <button type="button" onClick={reset} aria-label="처음 배율로">
            ⤾
          </button>
          <span className="chart-zoom-k">×{view.k.toFixed(1)}</span>
        </div>
      )}

      {width > 0 && (
        <svg
          ref={svgRef}
          className={interactive ? "chart-svg is-interactive" : "chart-svg"}
          width={width}
          height={height}
          role="img"
          aria-label={`${xLabel} × ${yLabel} 산점도`}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          onPointerLeave={() => setHoverId(null)}
          onDoubleClick={interactive ? reset : undefined}
        >
          <defs>
            <clipPath id={clipId}>
              <rect x={M.left} y={M.top} width={innerW} height={innerH} />
            </clipPath>
          </defs>

          {/* 격자 · 눈금 */}
          <g className="chart-grid">
            {xTicks.map((t) => {
              const px = sx(t);
              if (px < M.left - 0.5 || px > M.left + innerW + 0.5) return null;
              return (
                <g key={`x${t}`}>
                  <line x1={px} y1={M.top} x2={px} y2={M.top + innerH} />
                  <text className="chart-tick" x={px} y={M.top + innerH + 15} textAnchor="middle">
                    {fmtTick(t)}
                  </text>
                </g>
              );
            })}
            {yTicks.map((t) => {
              const py = sy(t);
              if (py < M.top - 0.5 || py > M.top + innerH + 0.5) return null;
              return (
                <g key={`y${t}`}>
                  <line x1={M.left} y1={py} x2={M.left + innerW} y2={py} />
                  <text className="chart-tick" x={M.left - 8} y={py + 3.5} textAnchor="end">
                    {fmtTick(t)}
                  </text>
                </g>
              );
            })}
          </g>
          <rect className="chart-frame" x={M.left} y={M.top} width={innerW} height={innerH} />

          <g clipPath={`url(#${clipId})`}>
            {empty ? null : connect && linePath ? <path className="chart-path" d={linePath} /> : null}

            {mode === "dot"
              ? dotPaths.map((dp) => (
                  <path
                    key={dp.group || "_"}
                    className="chart-dots"
                    d={dp.d}
                    // CSS 변수는 표현 속성이 아니라 style로 넣어야 확실히 산다.
                    style={{ stroke: color(dp.group || undefined) }}
                    strokeWidth={Math.min(7, 3.2 + view.k * 0.25)}
                  />
                ))
              : placed.map(({ p, cx, cy }) => {
                  const on = p.id === selectedId;
                  const hot = p.id === hoverId;
                  if (connect) {
                    // 패널 1 — 타일이 아니라 표식이다.
                    return (
                      <g key={p.id} className={p.emphasis ? "chart-node is-emphasis" : "chart-node"}>
                        <circle cx={cx} cy={cy} r={p.emphasis ? 6 : 3.5} style={{ fill: color(p.group) }} />
                        {p.emphasis && <circle className="chart-halo" cx={cx} cy={cy} r={11} />}
                        {showLabels && p.label && (
                          <text className="chart-point-label" x={cx + 9} y={cy - 7}>
                            {p.label}
                          </text>
                        )}
                        <title>{p.label ?? p.id}</title>
                      </g>
                    );
                  }
                  return (
                    <g key={p.id} className={on ? "chart-tile is-on" : hot ? "chart-tile is-hot" : "chart-tile"}>
                      <rect x={cx - half} y={cy - half} width={tileSize} height={tileSize} rx={3} />
                      {p.thumb ? (
                        <image
                          href={p.thumb}
                          x={cx - half + 1}
                          y={cy - half + 1}
                          width={tileSize - 2}
                          height={tileSize - 2}
                          preserveAspectRatio="xMidYMid slice"
                        />
                      ) : (
                        <text className="chart-tile-text" x={cx - half + 4} y={cy - half + 12}>
                          {wrapChars(p.text ?? p.label ?? "", 8, 5).map((ln, i) => (
                            <tspan key={i} x={cx - half + 4} dy={i === 0 ? 0 : 10.5}>
                              {ln}
                            </tspan>
                          ))}
                        </text>
                      )}
                      <rect
                        className="chart-tile-edge"
                        x={cx - half}
                        y={cy - half}
                        width={tileSize}
                        height={tileSize}
                        rx={3}
                        style={{ stroke: color(p.group) }}
                      />
                      <title>{p.text ?? p.label ?? p.id}</title>
                    </g>
                  );
                })}

            {/* 점 모드에서도 고른 항목은 보여야 한다 */}
            {mode === "dot" &&
              placed
                .filter(({ p }) => p.id === selectedId || p.id === hoverId)
                .map(({ p, cx, cy }) => (
                  <circle
                    key={`sel-${p.id}`}
                    className={p.id === selectedId ? "chart-dot-sel" : "chart-dot-hot"}
                    cx={cx}
                    cy={cy}
                    r={p.id === selectedId ? 8 : 5.5}
                  />
                ))}
          </g>

          <text className="chart-axis-label" x={M.left + innerW / 2} y={height - 6} textAnchor="middle">
            {xLabel}
          </text>
          <text
            className="chart-axis-label"
            transform={`translate(13 ${M.top + innerH / 2}) rotate(-90)`}
            textAnchor="middle"
          >
            {yLabel}
          </text>

          {empty && (
            <text className="chart-empty" x={M.left + innerW / 2} y={M.top + innerH / 2} textAnchor="middle">
              {emptyMessage}
            </text>
          )}
        </svg>
      )}
    </div>
  );
}
