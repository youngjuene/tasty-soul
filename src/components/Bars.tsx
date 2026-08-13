/**
 * 가로 막대 — **직접 쓴 인라인 SVG** (§2 · §20.8).
 *
 * §13 화면 5 패널 3: `|axes_change[a]|` 상위 3개.
 * `axes_change`는 커맨드가 준 값이다. 여기서 축 값을 계산하지 않는다 (§2 · §12.1).
 *
 * `null`은 `—`로 렌더한다 (§R10). 0으로 대체하지 않는다.
 */

import { dashSigned, EM_DASH } from "../lib/types";

export interface BarDatum {
  key: string;
  label: string;
  /** `null`이면 막대를 그리지 않고 `—`를 쓴다 (§R10). */
  value: number | null;
  /** 마우스를 올렸을 때 보일 설명. */
  hint?: string;
}

export interface BarsProps {
  data: BarDatum[];
  /** 0을 가운데 두고 좌우로 뻗는다. 부호 있는 변화량용. */
  diverging?: boolean;
  /** 축 최대 절대값. 없으면 데이터에서 잡되 최소 0.05를 둔다. */
  domainMax?: number;
  /** 값 포맷. 기본은 부호 붙은 소수 둘째 자리 (§8.2.1). */
  format?: (v: number) => string;
}

const W = 420;
const ROW_H = 34;
const LABEL_W = 86;
const PAD_T = 10;
const PAD_B = 22;
const TRACK_X = LABEL_W + 8;
const TRACK_W = W - TRACK_X - 12;
/** 막대 끝에 붙는 값 글자가 밖으로 밀려나지 않도록 남기는 여백. */
const VALUE_GUTTER = 46;

export default function Bars({ data, diverging = true, domainMax, format = dashSigned }: BarsProps) {
  const values = data.map((d) => d.value).filter((v): v is number => v !== null);
  const auto = values.length > 0 ? Math.max(...values.map(Math.abs)) : 0;
  const max = Math.max(domainMax ?? auto, 0.05);

  const H = PAD_T + data.length * ROW_H + PAD_B;
  const zeroX = diverging ? TRACK_X + TRACK_W / 2 : TRACK_X;
  const halfW = Math.max(20, (diverging ? TRACK_W / 2 : TRACK_W) - VALUE_GUTTER);

  return (
    <div className="chart-bars">
      <svg
        className="chart-svg"
        viewBox={`0 0 ${W} ${H}`}
        style={{ width: "100%", height: "auto" }}
        role="img"
        aria-label="축 변화량 막대"
      >
        {/* 0선 */}
        <line className="bar-zero" x1={zeroX} y1={PAD_T} x2={zeroX} y2={PAD_T + data.length * ROW_H} />

        {data.map((d, i) => {
          const y = PAD_T + i * ROW_H;
          const cy = y + ROW_H / 2;
          const v = d.value;
          const len = v === null ? 0 : (Math.abs(v) / max) * halfW;
          const neg = v !== null && v < 0;
          const bx = v === null ? zeroX : neg ? zeroX - len : zeroX;
          return (
            <g key={d.key} className={`bar-row${v === null ? " is-null" : neg ? " is-neg" : " is-pos"}`}>
              <text className="bar-label" x={LABEL_W} y={cy + 4} textAnchor="end">
                {d.label}
              </text>
              {v === null ? (
                <text className="bar-null" x={zeroX + 10} y={cy + 4}>
                  {EM_DASH}
                </text>
              ) : (
                <>
                  <rect className="bar-fill" x={bx} y={cy - 8} width={Math.max(1.5, len)} height={16} rx={2} />
                  <text
                    className="bar-value"
                    x={neg ? bx - 7 : bx + len + 7}
                    y={cy + 4}
                    textAnchor={neg ? "end" : "start"}
                  >
                    {format(v)}
                  </text>
                </>
              )}
              {d.hint && <title>{d.hint}</title>}
            </g>
          );
        })}

        {diverging && (
          <text className="bar-scale" x={zeroX - halfW} y={H - 8} textAnchor="start">
            {`−${max.toFixed(2)}`}
          </text>
        )}
        <text className="bar-scale" x={zeroX} y={H - 8} textAnchor={diverging ? "middle" : "start"}>
          0
        </text>
        <text className="bar-scale" x={zeroX + halfW} y={H - 8} textAnchor="end">
          {`+${max.toFixed(2)}`}
        </text>
      </svg>
    </div>
  );
}
