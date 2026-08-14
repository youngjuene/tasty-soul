/**
 * 화면 5 — 대시보드 (§13 화면 5).
 *
 * 패널 3개. **모든 데이터는 §12에서 이미 정의된 값만 쓴다.**
 * 이 화면은 순수 뷰다 — 축 값을 계산하거나 셀을 판정하지 않는다 (§2 · §12.1).
 *
 *   1. 변화 × 구체화   `timeline`의 `(drift, crystal)` 연결 산점도
 *   2. 2×2 셀 분포     `cells` / `cells_recent30`
 *   3. 최근 이동 축    `|axes_change[a]|` 상위 3개
 *
 * 관측이 부족해 패널을 그릴 수 없으면 **빈 상태**를 표시한다.
 * 축을 0으로 채워 가짜 차트를 그리지 않는다 (§13 화면 5).
 *
 * 실수 지표(divergence)는 대시보드에 싣지 않는다 (§12.6).
 * `null`은 `—`로 렌더한다 (§R10).
 */

import { useEffect, useMemo, useState } from "react";
import { dashboard, errorText } from "../lib/api";
import {
  AXES,
  AXIS_LABELS,
  AXIS_POLES,
  CELL_LABELS,
  dash,
  dashDate,
  EM_DASH,
  TERMS,
} from "../lib/types";
import type { CellName, Derived, MonthState, PromptBoundary } from "../lib/types";
import Scatter from "../components/Scatter";
import type { ScatterPoint } from "../components/Scatter";
import CellGrid from "../components/CellGrid";
import Bars from "../components/Bars";
import type { BarDatum } from "../components/Bars";
import Explain, { Ref } from "../components/Explain";
import "../styles/charts.css";

// ────────────────────────────────────────── ULID → 월 (§R11 경계 배치용)

const CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/**
 * ULID 앞 10자는 밀리초 타임스탬프다. 경계를 **시간 축 위에** 놓기 위해 읽는다.
 * 파생값 계산이 아니라 식별자 해독이다 — 판단은 하지 않는다.
 */
function ulidMonth(id: string): string | null {
  if (id.length < 10) return null;
  let ms = 0;
  for (let i = 0; i < 10; i++) {
    const v = CROCKFORD.indexOf(id[i]!.toUpperCase());
    if (v < 0) return null;
    ms = ms * 32 + v;
  }
  const d = new Date(ms);
  if (Number.isNaN(d.getTime())) return null;
  return `${d.getUTCFullYear()}-${String(d.getUTCMonth() + 1).padStart(2, "0")}`;
}

// ───────────────────────────────────────────────────────── 시간 축 스트립

const STRIP_W = 640;
const STRIP_H = 52;
const STRIP_PAD = 26;

/**
 * 패널 1의 경로가 지나온 **시간**을 따로 편다.
 * 패널 1 자체는 `(drift, crystal)` 평면이라 시간 축이 없다 —
 * §R11이 요구하는 `prompt_sha256` 경계의 세로 구분선은 여기에 긋는다.
 */
function MonthStrip({ timeline, boundaries }: { timeline: MonthState[]; boundaries: PromptBoundary[] }) {
  const months = timeline.map((m) => m.month);
  const n = months.length;
  if (n === 0) return null;

  const step = n > 1 ? (STRIP_W - STRIP_PAD * 2) / (n - 1) : 0;
  const px = (i: number) => (n > 1 ? STRIP_PAD + i * step : STRIP_W / 2);

  // 라벨이 겹치지 않을 만큼만 쓴다.
  const every = Math.max(1, Math.ceil(n / 7));

  // 같은 달에 여러 경계가 있으면 한 줄로 묶고 개수를 쓴다.
  // 종류(`ingest`=describe.md / `soul_delta`=reflect.md)를 함께 적는다 — 두 해시는
  // 서로 다른 파일의 것이라 종류 없이 나란히 두면 같은 축으로 읽힌다 (§R11).
  const marks = new Map<number, string[]>();
  for (const b of boundaries) {
    const m = ulidMonth(b.id);
    if (!m) continue;
    let idx = months.indexOf(m);
    if (idx < 0) {
      idx = months.findIndex((mm) => mm >= m);
      if (idx < 0) continue; // 타임라인 이후 — 그릴 자리가 없다
    }
    const arr = marks.get(idx) ?? [];
    arr.push(`${b.kind} ${b.sha256.slice(0, 6)}`);
    marks.set(idx, arr);
  }

  return (
    <svg
      className="chart-svg strip"
      viewBox={`0 0 ${STRIP_W} ${STRIP_H}`}
      style={{ width: "100%", height: "auto" }}
      role="img"
      aria-label="달 축과 프롬프트 경계"
    >
      <line className="strip-axis" x1={STRIP_PAD - 8} y1={26} x2={STRIP_W - STRIP_PAD + 8} y2={26} />

      {[...marks.entries()].map(([idx, shas]) => {
        const x = Math.max(6, Math.min(STRIP_W - 6, px(idx) - (n > 1 ? step / 2 : 0)));
        return (
          <g key={`b${idx}`} className="strip-boundary">
            <line x1={x} y1={6} x2={x} y2={44} />
            <text x={x + 4} y={13}>
              질문지 바뀜 {shas.length > 1 ? `${shas.length}회` : ""}
            </text>
            <title>{`기계에 주는 질문지가 이 지점에서 바뀌었습니다 (prompt_sha256 · §R11)\n${shas.join(" · ")}\n이 뒤의 움직임은 취향이 아니라 질문이 바뀐 탓일 수 있습니다`}</title>
          </g>
        );
      })}

      {timeline.map((m, i) => {
        const drawn = m.drift !== null && m.crystal !== null;
        return (
          <g key={m.month} className={drawn ? "strip-tick" : "strip-tick is-skipped"}>
            <circle cx={px(i)} cy={26} r={drawn ? 3 : 2.5} />
            {(i % every === 0 || i === n - 1) && (
              <text x={px(i)} y={44} textAnchor="middle">
                {m.month}
              </text>
            )}
            <title>{`${m.month} · 누적 ${m.n}건 · 변화 ${dash(m.drift)} · 또렷함 ${dash(m.crystal)}`}</title>
          </g>
        );
      })}
    </svg>
  );
}

// ─────────────────────────────────────────────────────────────── 본체

export interface DashboardProps {
  /** 아카이브로 넘어가는 경로 (§13 화면 6 — "대시보드에서 넘어가는 공간"). */
  onOpenArchive?: (cell?: CellName) => void;
}

type Load =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ok"; derived: Derived };

export default function Dashboard({ onOpenArchive }: DashboardProps = {}) {
  const [load, setLoad] = useState<Load>({ status: "loading" });

  useEffect(() => {
    let alive = true;
    dashboard()
      .then((derived) => alive && setLoad({ status: "ok", derived }))
      .catch((e: unknown) => alive && setLoad({ status: "error", message: errorText(e) }));
    return () => {
      alive = false;
    };
  }, []);

  const derived = load.status === "ok" ? load.derived : null;

  // 패널 1 — 둘 중 하나라도 null인 달은 건너뛰되 선은 끊지 않는다 (§13 화면 5).
  const pathPoints: ScatterPoint[] = useMemo(() => {
    if (!derived) return [];
    const current = derived.timeline.length > 0 ? derived.timeline[derived.timeline.length - 1]!.month : null;
    return derived.timeline
      .filter((m) => m.drift !== null && m.crystal !== null)
      .map((m) => ({
        id: m.month,
        x: m.drift as number,
        y: m.crystal as number,
        label: m.month,
        emphasis: m.month === current,
      }));
  }, [derived]);

  // 패널 3 — |axes_change[a]| 상위 3개. 값은 커맨드가 줬다.
  const { topBars, nullAxes } = useMemo(() => {
    if (!derived) return { topBars: [] as BarDatum[], nullAxes: [] as string[] };
    const rows = AXES.map((axis, i) => ({ axis, value: derived.axes_change[i] ?? null }));
    const known = rows.filter((r) => r.value !== null);
    known.sort((a, b) => Math.abs(b.value as number) - Math.abs(a.value as number));
    return {
      topBars: known.slice(0, 3).map<BarDatum>((r) => ({
        key: r.axis,
        label: AXIS_LABELS[r.axis],
        value: r.value,
        hint: `${AXIS_LABELS[r.axis]} (${r.axis})\n0 · ${AXIS_POLES[r.axis][0]}\n1 · ${AXIS_POLES[r.axis][1]}`,
      })),
      nullAxes: rows.filter((r) => r.value === null).map((r) => r.axis as string),
    };
  }, [derived]);

  if (load.status === "loading") {
    return (
      <div className="dash">
        <p className="dash-note">읽는 중…</p>
      </div>
    );
  }

  if (load.status === "error") {
    return (
      <div className="dash">
        <div className="chart-empty-box">
          <p>대시보드를 읽지 못했습니다.</p>
          <p className="chart-empty-sub">{load.message}</p>
        </div>
      </div>
    );
  }

  const d = load.derived;
  const hasObservations = d.observation_count > 0 && d.t_ref !== null;

  if (!hasObservations) {
    return (
      <div className="dash">
        <header className="dash-head">
          <h1>대시보드</h1>
        </header>
        <div className="chart-empty-box">
          <p>아직 아무것도 없습니다.</p>
          <p className="chart-empty-sub">
            «투입»에서 사진이든 링크든 하나 넣고 카드에 답하면 여기부터 쌓이기 시작합니다.
          </p>
        </div>
      </div>
    );
  }

  const boundaries = d.prompt_boundaries ?? [];

  return (
    <div className="dash">
      <header className="dash-head">
        <h1>대시보드</h1>
        <p className="dash-lead">
          지금까지 답한 것을 모아 놓은 자리입니다. 넣을수록 채워지고, 처음에는 대부분 비어 있습니다.
        </p>
        <dl className="dash-meta">
          <div>
            <dt>기준일</dt>
            <dd>{dashDate(d.t_ref)}</dd>
          </div>
          <div>
            <dt>넣은 것</dt>
            <dd>{d.observation_count}</dd>
          </div>
          <div>
            <dt>시작</dt>
            <dd>{dashDate(d.t_first)}</dd>
          </div>
          <div>
            <dt>어긋남</dt>
            <dd>{dash(d.misread_ratio)}</dd>
          </div>
          {/* `crystal` 은 한 이름으로만 부른다. 패널 1도 "또렷함"이다. */}
          <div>
            <dt>또렷함</dt>
            <dd>{dash(d.crystal_now)}</dd>
          </div>
        </dl>
        {/* 라벨이 "다섯"이라고 했으면 다섯 개가 다 있어야 한다. 셋만 풀면 나머지 둘은
            설명할 가치가 없다는 뜻으로 읽힌다. */}
        <Explain
          label="이 다섯 숫자가 무슨 뜻인가요"
          items={[TERMS.tRef, TERMS.count, TERMS.first, TERMS.misread, TERMS.crystal]}
        />
      </header>

      <div className="dash-grid">
        {/* ───────────────────────── 패널 1 */}
        <section className="panel panel-wide">
          <h2>얼마나 움직였나 · 얼마나 또렷해졌나</h2>
          <p className="panel-sub">
            점 하나가 한 달입니다. 가로는 지난달과 견줘 취향이 <b>얼마나 움직였는지</b>, 세로는
            취향의 윤곽이 <b>얼마나 또렷해졌는지</b>. 오래된 달부터 순서대로 이었습니다.
          </p>
          {pathPoints.length === 0 ? (
            <div className="chart-empty-box">
              <p>아직 그릴 만큼 쌓이지 않았습니다.</p>
              <p className="chart-empty-sub">
                한 달에 3건 이상 쌓이면 가로가, 비슷한 것끼리 무리가 지어지면 세로가 나옵니다.
              </p>
            </div>
          ) : (
            <>
              <Scatter
                points={pathPoints}
                xLabel="변화 → drift"
                yLabel="또렷함 → crystal"
                height={320}
                connect
                showLabels
                emptyMessage="아직 그릴 만큼 쌓이지 않았습니다"
              />
              <MonthStrip timeline={d.timeline} boundaries={boundaries} />
              <p className="panel-foot">
                {`달 ${d.timeline.length}개 중 ${pathPoints.length}개를 찍었습니다. 나머지 달은 둘 중 하나가 ${EM_DASH}(아직 잴 수 없음)입니다.`}
              </p>
            </>
          )}
          <Explain items={[TERMS.drift, TERMS.crystal, TERMS.cluster]} />
        </section>

        {/* ───────────────────────── 패널 2 */}
        <section className="panel">
          <h2>네 칸에 어떻게 나뉘었나</h2>
          <p className="panel-sub">
            한 항목에 카드 두 장 모두 답하면 그 답의 조합에 따라 네 칸 중 하나에 들어갑니다. 이 앱이
            특히 찾는 칸은 <b>{CELL_LABELS.other_reason}</b> — 보는 건 같은데 끌린 이유가 달랐던
            것입니다.
          </p>
          <CellGrid
            cells={d.cells}
            recent30={d.cells_recent30}
            observationCount={d.observation_count}
            onSelect={onOpenArchive ? (cell) => onOpenArchive(cell) : undefined}
          />
        </section>

        {/* ───────────────────────── 패널 3 */}
        <section className="panel">
          <h2>요즘 가장 많이 움직인 것</h2>
          <p className="panel-sub">최근 90일을 그 앞 90일과 견줍니다. 가장 많이 움직인 셋.</p>
          {topBars.length === 0 ? (
            <div className="chart-empty-box">
              <p>아직 견줄 만큼 쌓이지 않았습니다.</p>
            </div>
          ) : (
            <>
              <Bars data={topBars} diverging />
              {nullAxes.length > 0 && (
                <p className="panel-foot">
                  {`아직 잴 수 없음 ${EM_DASH} ${nullAxes.map((a) => AXIS_LABELS[a as keyof typeof AXIS_LABELS] ?? a).join(" · ")}`}
                </p>
              )}
            </>
          )}
          {/*
            §R11 — 이 문장은 줄이지 않는다 (`ui/README.md` 6). 다만 "프롬프트 경계"가
            무엇인지 모르면 경고가 경고로 읽히지 않으므로, 그 말을 먼저 푼다.
          */}
          {boundaries.length > 0 && (
            <p className="panel-warn">
              중간에 기계가 쓰는 질문지가 {boundaries.length}번 바뀌었습니다. 이 움직임의 일부는
              취향이 변한 것이 아니라 <b>질문이 바뀐 탓</b>일 수 있습니다.
              <Ref>§R11</Ref>
            </p>
          )}
          <Explain
            label="축 이름이 무슨 뜻인가요"
            items={AXES.map((a) => ({
              name: AXIS_LABELS[a],
              id: a,
              gloss: `0 · ${AXIS_POLES[a][0]} ↔ 1 · ${AXIS_POLES[a][1]}`,
            }))}
          />
        </section>
      </div>

      {onOpenArchive && (
        <div className="dash-more">
          <button type="button" className="btn" onClick={() => onOpenArchive()}>
            숫자 말고 넣은 것들을 하나씩 보기 →
          </button>
        </div>
      )}
    </div>
  );
}
