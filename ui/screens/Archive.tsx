/**
 * 화면 6 — 아카이브 탐색 (§13 화면 6).
 *
 * 대시보드에서 넘어가는 공간이다. 통계가 아니라 **투입한 것들 자체**를 훑는다.
 *
 * - 기본은 두 감각 축 산점도 (기본 `grain` × `valence`). 부가로 구조 보기(PCA `px`/`py`).
 * - 타일은 썸네일. 없으면 서술문 앞 40자를 렌더한다 (T70c).
 * - 밀도: 보이는 항목 ≤ 200 → 타일, 초과 → 단색 점. 확대하면 타일로 전환 (T67).
 * - 패싯 필터·검색은 **전부 백엔드 SQL**이다. 프런트에서 다시 계산하지 않는다.
 *   어떤 필터도 API를 호출하지 않는다 (T68).
 * - 검색은 부분 문자열 일치만. 의미 검색 UI를 만들지 않는다 (§13 화면 6 · §19.5).
 * - 이웃 탐색은 캐시된 벡터끼리 코사인 (T33).
 * - 항목 상세에서 **미응답 층에 그 자리에서 답한다** — 화면 2·2.1을 다시 띄우지 않는다 (T70).
 *
 * supersede된 `ingest`는 커맨드가 이미 걸러 준다 (§R9 · T70b).
 * 첫 렌더 500ms 예산 (T67) — 5,000건의 타일을 DOM에 넣지 않는다. `Scatter`가 처리한다.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  archiveDetail,
  archiveNeighbors,
  archiveQuery,
  errorText,
  recastKind,
  recastPreview,
  recordReading,
  redoContext,
} from "../lib/api";
import {
  AXES,
  AXIS_LABELS,
  AXIS_POLES,
  axisLabel,
  CELL_INCOMPLETE_LABEL,
  CELL_LABELS,
  CELL_MEANINGS,
  cellLabel,
  CELLS,
  DEFAULT_X_AXIS,
  DEFAULT_Y_AXIS,
  dash,
  EM_DASH,
  emptyArchiveQuery,
  kindLabel,
  KINDS,
  NOT_GROUNDED_NOTICE,
  QUALITIES,
  QUALITY_MEANINGS,
  qualityLabel,
  TERMS,
  TILE_PROSE_CHARS,
  TILE_RENDER_LIMIT,
  VERDICT_LABELS,
} from "../lib/types";
import type {
  ArchiveItem,
  ArchiveQuery,
  AxisName,
  CellName,
  ItemDetail,
  Kind,
  Layer,
  ReadingView,
} from "../lib/types";
import Scatter from "../components/Scatter";
import type { ScatterMode, ScatterPoint } from "../components/Scatter";
import Explain from "../components/Explain";
import "../styles/charts.css";

/**
 * §13 화면 6의 셀 패싯. 다섯 번째는 표의 "미완성"이다 —
 * 두 층 중 한쪽이 미응답이거나 문화 글귀가 없어 `cell === null`인 항목 (§12.6).
 *
 * ⚠ `"incomplete"`는 백엔드가 `ArchiveQuery.cells`에서 이 뜻으로 받아야 하는 값이다.
 *   Rust `Cell::parse`는 네 값만 알므로 SQL 쪽에서 이 하나를 따로 다뤄야 한다.
 */
const CELL_INCOMPLETE = "incomplete";

const INCOMPLETE_TITLE = "카드 두 장 중 한쪽에 아직 답하지 않았거나, 문화 글귀가 없습니다";

const CELL_FACETS: { key: string; label: string; title: string }[] = [
  ...CELLS.map((c) => ({
    key: c as string,
    label: CELL_LABELS[c],
    title: `${CELL_LABELS[c]} (${c})\n${CELL_MEANINGS[c]}`,
  })),
  {
    key: CELL_INCOMPLETE,
    label: CELL_INCOMPLETE_LABEL,
    title: INCOMPLETE_TITLE,
  },
];

function cellTitle(cell: string | null): string {
  if (cell === null) return INCOMPLETE_TITLE;
  const meaning = CELL_MEANINGS[cell as CellName];
  return meaning ? `${cellLabel(cell)} (${cell})\n${meaning}` : cell;
}

function colorForCell(group: string | undefined): string {
  switch (group) {
    case "read":
      return "var(--ch-cell-read)";
    case "other_reason":
      return "var(--ch-cell-other)";
    case "wrong_words":
      return "var(--ch-cell-wrong)";
    case "unread":
      return "var(--ch-cell-unread)";
    default:
      return "var(--ch-cell-incomplete)";
  }
}

function toggle<T>(set: T[], v: T): T[] {
  return set.includes(v) ? set.filter((x) => x !== v) : [...set, v];
}

// ────────────────────────────────────────────────────────────── 본체

export interface ArchiveProps {
  /** 대시보드에서 셀을 눌러 넘어올 때의 초기 패싯. */
  initialCell?: string;
}

export default function Archive({ initialCell }: ArchiveProps = {}) {
  // ── 패싯 상태
  const [kinds, setKinds] = useState<string[]>([]);
  const [cells, setCells] = useState<string[]>(initialCell ? [initialCell] : []);
  const [cluster, setCluster] = useState<number | null>(null);
  const [sMin, setSMin] = useState(0);
  const [sMax, setSMax] = useState(1);
  const [months, setMonths] = useState<string[]>([]);
  const [tags, setTags] = useState<string[]>([]);
  const [qualities, setQualities] = useState<string[]>([]);
  const [search, setSearch] = useState("");

  // ── 보기 상태
  const [xAxis, setXAxis] = useState<AxisName>(DEFAULT_X_AXIS);
  const [yAxis, setYAxis] = useState<AxisName>(DEFAULT_Y_AXIS);
  const [view, setView] = useState<"axes" | "structure">("axes");
  const [tagFilter, setTagFilter] = useState("");

  // ── 데이터
  const [base, setBase] = useState<ArchiveItem[] | null>(null);
  const [items, setItems] = useState<ArchiveItem[]>([]);
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [visible, setVisible] = useState<{ n: number; mode: ScatterMode }>({ n: 0, mode: "tile" });
  /** 상세에서 응답·재시도·뒤집기가 일어나면 셀이 바뀐다. 다시 묻는다 — 판정은 백엔드가 한다. */
  const [refresh, setRefresh] = useState(0);

  // 모든 키가 채워져야 한다 — Rust `ArchiveQuery`는 `#[serde(default)]`가 아니다.
  const query: ArchiveQuery = useMemo(
    () => ({
      ...emptyArchiveQuery(),
      kinds,
      cells,
      cluster,
      surprisal_min: sMin > 0 ? sMin : null,
      surprisal_max: sMax < 1 ? sMax : null,
      months,
      tags,
      qualities,
      search: search.trim() === "" ? null : search.trim(),
      x_axis: xAxis,
      y_axis: yAxis,
    }),
    [kinds, cells, cluster, sMin, sMax, months, tags, qualities, search, xAxis, yAxis],
  );

  /** 대시보드에서 셀을 눌러 넘어왔으면 처음부터 필터가 걸려 있다. */
  const startsFiltered = useRef(initialCell !== undefined).current;

  // 최초 1회 — 필터 없는 전체. 패싯 후보(달·태그·군집)를 여기서 읽는다.
  // 필터링 자체는 언제나 백엔드 SQL이다 (§13 화면 6). 이건 **선택지 목록**용이다.
  useEffect(() => {
    let alive = true;
    archiveQuery({ ...emptyArchiveQuery(), x_axis: DEFAULT_X_AXIS, y_axis: DEFAULT_Y_AXIS })
      .then((rows) => {
        if (!alive) return;
        setBase(rows);
        if (!startsFiltered) {
          setItems(rows);
          setBusy(false);
        }
      })
      .catch((e: unknown) => {
        if (!alive) return;
        setError(errorText(e));
        setBusy(false);
      });
    return () => {
      alive = false;
    };
  }, [startsFiltered]);

  // 필터가 바뀌면 다시 묻는다. **전부 백엔드 SQL이다** (§13 화면 6 · T68).
  const firstRun = useRef(!startsFiltered);
  useEffect(() => {
    if (firstRun.current) {
      firstRun.current = false;
      return;
    }
    let alive = true;
    setBusy(true);
    const t = window.setTimeout(() => {
      archiveQuery(query)
        .then((rows) => {
          if (!alive) return;
          setItems(rows);
          setBusy(false);
        })
        .catch((e: unknown) => {
          if (!alive) return;
          setError(errorText(e));
          setBusy(false);
        });
    }, 160);
    return () => {
      alive = false;
      window.clearTimeout(t);
    };
  }, [query, refresh]);

  // ── 패싯 후보. 필터가 아니라 **선택지 목록**이다.
  const facetMonths = useMemo(() => {
    const s = new Set<string>();
    for (const it of base ?? []) s.add(it.month);
    return [...s].sort().reverse();
  }, [base]);

  const facetTags = useMemo(() => {
    const m = new Map<string, number>();
    for (const it of base ?? []) for (const t of it.tags) m.set(t, (m.get(t) ?? 0) + 1);
    return [...m.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
  }, [base]);

  const facetClusters = useMemo(() => {
    const s = new Set<number>();
    for (const it of base ?? []) if (it.cluster !== null && it.cluster !== undefined) s.add(it.cluster);
    return [...s].sort((a, b) => a - b);
  }, [base]);

  // ── 산점도 입력
  const structureDropped = useMemo(
    () => (view === "structure" ? items.filter((i) => i.px === null || i.py === null).length : 0),
    [view, items],
  );

  const points: ScatterPoint[] = useMemo(() => {
    const out: ScatterPoint[] = [];
    for (const it of items) {
      let x: number;
      let y: number;
      if (view === "structure") {
        if (it.px === null || it.px === undefined || it.py === null || it.py === undefined) continue;
        x = it.px;
        y = it.py;
      } else {
        x = it.x;
        y = it.y;
      }
      out.push({
        id: it.id,
        x,
        y,
        thumb: it.thumb_data_url,
        // T70c — 썸네일이 없으면 서술문 앞 40자
        text: it.prose.slice(0, TILE_PROSE_CHARS),
        group: it.cell ?? "incomplete",
      });
    }
    return out;
  }, [items, view]);

  const onVisibleChange = useCallback((n: number, mode: ScatterMode) => {
    setVisible((v) => (v.n === n && v.mode === mode ? v : { n, mode }));
  }, []);

  const resetFacets = () => {
    setKinds([]);
    setCells([]);
    setCluster(null);
    setSMin(0);
    setSMax(1);
    setMonths([]);
    setTags([]);
    setQualities([]);
    setSearch("");
  };

  const facetCount =
    kinds.length +
    cells.length +
    months.length +
    tags.length +
    qualities.length +
    (cluster === null ? 0 : 1) +
    (sMin > 0 || sMax < 1 ? 1 : 0) +
    (search.trim() ? 1 : 0);

  if (error) {
    return (
      <div className="arc">
        <div className="chart-empty-box">
          <p>아카이브를 읽지 못했습니다.</p>
          <p className="chart-empty-sub">{error}</p>
        </div>
      </div>
    );
  }

  return (
    <div className="arc">
      {/* ────────────────────────────── 패싯 */}
      <aside className="arc-facets">
        <div className="facet-head">
          <h2>좁히기</h2>
          {facetCount > 0 && (
            <button type="button" className="btn btn-quiet" onClick={resetFacets}>
              {`모두 풀기 (${facetCount})`}
            </button>
          )}
        </div>

        <label className="facet-search">
          <span className="facet-title">검색</span>
          <input
            type="search"
            value={search}
            placeholder="기계의 서술 · 문화 글귀 · 내가 쓴 정정 · 태그"
            onChange={(e) => setSearch(e.target.value)}
          />
          <span className="facet-note">글자가 그대로 들어 있는 것만 찾습니다.</span>
        </label>

        <fieldset className="facet">
          <legend>종류</legend>
          {KINDS.map((k) => (
            <label key={k} title={k}>
              <input type="checkbox" checked={kinds.includes(k)} onChange={() => setKinds(toggle(kinds, k))} />
              {kindLabel(k)}
            </label>
          ))}
        </fieldset>

        <fieldset className="facet">
          <legend>카드 두 장의 답</legend>
          {CELL_FACETS.map((c) => (
            <label key={c.key} title={c.title}>
              <input
                type="checkbox"
                checked={cells.includes(c.key)}
                onChange={() => setCells(toggle(cells, c.key))}
              />
              <span className={`cell-swatch sw-${c.key}`} aria-hidden="true" />
              {c.label}
            </label>
          ))}
        </fieldset>

        <fieldset className="facet">
          <legend>무리</legend>
          {facetClusters.length === 0 ? (
            <p className="facet-note">{`아직 무리가 지어지지 않았습니다 ${EM_DASH}`}</p>
          ) : (
            <select
              value={cluster === null ? "" : String(cluster)}
              onChange={(e) => setCluster(e.target.value === "" ? null : Number(e.target.value))}
              title={TERMS.cluster.gloss}
            >
              <option value="">전체</option>
              {facetClusters.map((c) => (
                <option key={c} value={String(c)}>
                  {`${c}번 무리`}
                </option>
              ))}
            </select>
          )}
        </fieldset>

        <fieldset className="facet">
          <legend>새로움</legend>
          <div className="facet-range">
            <input
              type="range"
              min={0}
              max={1}
              step={0.05}
              value={sMin}
              onChange={(e) => setSMin(Math.min(Number(e.target.value), sMax))}
              aria-label="새로움 하한"
            />
            <input
              type="range"
              min={0}
              max={1}
              step={0.05}
              value={sMax}
              onChange={(e) => setSMax(Math.max(Number(e.target.value), sMin))}
              aria-label="새로움 상한"
            />
          </div>
          <p className="facet-note">
            {`${sMin.toFixed(2)} – ${sMax.toFixed(2)} · 1에 가까울수록 처음 보는 것`}
          </p>
        </fieldset>

        <fieldset className="facet">
          <legend>기간</legend>
          <div className="facet-scroll">
            {facetMonths.length === 0 ? (
              <p className="facet-note">{EM_DASH}</p>
            ) : (
              facetMonths.map((m) => (
                <label key={m}>
                  <input type="checkbox" checked={months.includes(m)} onChange={() => setMonths(toggle(months, m))} />
                  {m}
                </label>
              ))
            )}
          </div>
        </fieldset>

        <fieldset className="facet">
          <legend>태그</legend>
          <input
            className="facet-tagfilter"
            type="search"
            value={tagFilter}
            placeholder="태그 좁히기"
            onChange={(e) => setTagFilter(e.target.value)}
          />
          <div className="facet-scroll">
            {facetTags.length === 0 ? (
              <p className="facet-note">{EM_DASH}</p>
            ) : (
              facetTags
                .filter(([t]) => tagFilter === "" || t.includes(tagFilter))
                .slice(0, 120)
                .map(([t, n]) => (
                  <label key={t}>
                    <input type="checkbox" checked={tags.includes(t)} onChange={() => setTags(toggle(tags, t))} />
                    {t}
                    <span className="facet-count">{n}</span>
                  </label>
                ))
            )}
          </div>
        </fieldset>

        <fieldset className="facet">
          <legend>얼마나 보고 썼나</legend>
          {QUALITIES.map((q) => (
            <label key={q} title={`${q} · ${QUALITY_MEANINGS[q]}`}>
              <input
                type="checkbox"
                checked={qualities.includes(q)}
                onChange={() => setQualities(toggle(qualities, q))}
              />
              {qualityLabel(q)}
            </label>
          ))}
        </fieldset>

        {/*
          좁히는 칸의 말들을 여기서 한 번에 푼다. 패널마다 흩어 놓으면 좁은
          사이드바가 설명으로 두 배가 된다.
        */}
        <Explain
          label="여기 말들이 무슨 뜻인가요"
          items={[
            TERMS.surprisal,
            TERMS.cluster,
            TERMS.quality,
            ...CELLS.map((c) => ({
              name: CELL_LABELS[c],
              id: c as string,
              gloss: CELL_MEANINGS[c],
            })),
            { name: CELL_INCOMPLETE_LABEL, id: "cell = null", gloss: INCOMPLETE_TITLE },
          ]}
        />
      </aside>

      {/* ────────────────────────────── 공간 */}
      <main className="arc-main">
        <header className="arc-bar">
          <div className="arc-view">
            <button
              type="button"
              className={view === "axes" ? "seg is-on" : "seg"}
              onClick={() => setView("axes")}
            >
              축 보기
            </button>
            <button
              type="button"
              className={view === "structure" ? "seg is-on" : "seg"}
              onClick={() => setView("structure")}
              title={TERMS.structure.gloss}
            >
              구조 보기
            </button>
          </div>

          {view === "axes" && (
            <div className="arc-axes">
              <label>
                가로
                <select value={xAxis} onChange={(e) => setXAxis(e.target.value as AxisName)}>
                  {AXES.map((a) => (
                    <option key={a} value={a} title={`${AXIS_LABELS[a]} (${a})\n0 · ${AXIS_POLES[a][0]}\n1 · ${AXIS_POLES[a][1]}`}>
                      {AXIS_LABELS[a]}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                세로
                <select value={yAxis} onChange={(e) => setYAxis(e.target.value as AxisName)}>
                  {AXES.map((a) => (
                    <option key={a} value={a} title={`${AXIS_LABELS[a]} (${a})\n0 · ${AXIS_POLES[a][0]}\n1 · ${AXIS_POLES[a][1]}`}>
                      {AXIS_LABELS[a]}
                    </option>
                  ))}
                </select>
              </label>
            </div>
          )}

          <div className="arc-counts">
            {busy ? (
              <span className="arc-busy">읽는 중…</span>
            ) : (
              <span>
                {`${items.length}건 중 화면에 ${visible.n}건`}
              </span>
            )}
          </div>
        </header>

        {visible.mode === "dot" && (
          <p className="arc-hint">{`한 화면에 ${TILE_RENDER_LIMIT}건이 넘어 점으로 그립니다. 확대하면 그림으로 바뀝니다.`}</p>
        )}
        {view === "structure" && structureDropped > 0 && (
          <p className="arc-hint">{`자리를 정할 수 없는 ${structureDropped}건은 이 보기에서 빠졌습니다.`}</p>
        )}
        {view === "structure" && (
          <p className="arc-hint">{TERMS.structure.gloss}</p>
        )}

        <Scatter
          points={points}
          xLabel={view === "structure" ? "PC1" : axisLabel(xAxis)}
          yLabel={view === "structure" ? "PC2" : axisLabel(yAxis)}
          height={520}
          interactive
          colorOf={colorForCell}
          selectedId={selectedId}
          onSelect={setSelectedId}
          onVisibleChange={onVisibleChange}
          tileThreshold={TILE_RENDER_LIMIT}
          xDomain={view === "axes" ? [0, 1] : undefined}
          yDomain={view === "axes" ? [0, 1] : undefined}
          emptyMessage={busy ? "읽는 중…" : "조건에 맞는 항목이 없습니다"}
        />

        <ul className="arc-legend">
          {CELL_FACETS.map((c) => (
            <li key={c.key}>
              <span className={`cell-swatch sw-${c.key}`} aria-hidden="true" />
              {c.label}
            </li>
          ))}
        </ul>
      </main>

      {/* ────────────────────────────── 상세 */}
      <aside className="arc-detail">
        {selectedId === null ? (
          <p className="arc-placeholder">
            점을 하나 고르면 기계가 쓴 두 글귀와 그때 내가 뭐라고 답했는지가 나란히 나옵니다.
          </p>
        ) : (
          <ItemDetailPanel
            key={selectedId}
            id={selectedId}
            onSelect={setSelectedId}
            onClose={() => setSelectedId(null)}
            onChanged={() => setRefresh((n) => n + 1)}
          />
        )}
      </aside>
    </div>
  );
}

// ───────────────────────────────────────────────────────── 항목 상세

function ItemDetailPanel({
  id,
  onSelect,
  onClose,
  onChanged,
}: {
  id: string;
  onSelect: (id: string) => void;
  onClose: () => void;
  /** 관측이 생겼다 — 목록의 셀 색이 바뀌므로 다시 묻게 한다. */
  onChanged: () => void;
}) {
  const [detail, setDetail] = useState<ItemDetail | null>(null);
  const [neighbors, setNeighbors] = useState<ArchiveItem[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [tick, setTick] = useState(0);
  const [showSources, setShowSources] = useState(false);
  const [recastAsk, setRecastAsk] = useState<number | null>(null);
  const [pendingAction, setPendingAction] = useState(false);

  const reload = useCallback(() => {
    setTick((t) => t + 1);
    onChanged();
  }, [onChanged]);

  useEffect(() => {
    let alive = true;
    setErr(null);
    archiveDetail(id)
      .then((d) => alive && setDetail(d))
      .catch((e: unknown) => alive && setErr(errorText(e)));
    return () => {
      alive = false;
    };
  }, [id, tick]);

  useEffect(() => {
    let alive = true;
    archiveNeighbors(id, 8)
      .then((r) => alive && setNeighbors(r))
      .catch(() => alive && setNeighbors([]));
    return () => {
      alive = false;
    };
  }, [id]);

  if (err) {
    return (
      <div className="detail">
        <p className="chart-empty-sub">{err}</p>
      </div>
    );
  }
  if (!detail) return <div className="detail"><p className="arc-placeholder">읽는 중…</p></div>;

  const it = detail.item;
  const ctx = detail.context ?? null;

  // §9.3 — YouTube 항목의 kind 뒤집기는 audio ↔ video 둘 사이다.
  const flipTo: Kind = it.kind === "video" ? "audio" : "video";

  const doRecast = async () => {
    setPendingAction(true);
    try {
      // §9.3 — 뒤집기는 새 ingest 를 만들고 이전 것을 supersede 한다. id 가 바뀐다.
      await recastKind(it.id, flipTo);
      setRecastAsk(null);
      onChanged();
      onClose();
    } catch (e) {
      setErr(errorText(e));
    } finally {
      setPendingAction(false);
    }
  };

  const askRecast = async () => {
    try {
      const n = await recastPreview(it.id);
      setRecastAsk(n);
    } catch (e) {
      setErr(errorText(e));
    }
  };

  const doRedo = async () => {
    setPendingAction(true);
    try {
      await redoContext(it.id);
      reload();
    } catch (e) {
      setErr(errorText(e));
    } finally {
      setPendingAction(false);
    }
  };

  return (
    <div className="detail">
      <header className="detail-head">
        <div className="detail-thumb">
          {it.thumb_data_url ? (
            <img src={it.thumb_data_url} alt="" />
          ) : (
            <span className="detail-thumb-text">{it.prose.slice(0, TILE_PROSE_CHARS)}</span>
          )}
        </div>
        <div className="detail-origin">
          <p className="detail-kind">{kindLabel(it.kind)}</p>
          <p className="detail-origin-text" title={detail.origin}>
            {detail.origin}
          </p>
        </div>
        <button type="button" className="btn btn-quiet detail-close" onClick={onClose} aria-label="닫기">
          ×
        </button>
      </header>

      <dl className="detail-facts">
        <div title={TERMS.surprisal.gloss}>
          <dt>{TERMS.surprisal.name}</dt>
          <dd>{dash(it.surprisal)}</dd>
        </div>
        <div title={TERMS.cluster.gloss}>
          <dt>{TERMS.cluster.name}</dt>
          <dd>
            {it.cluster === null || it.cluster === undefined ? EM_DASH : `${it.cluster}번`}
          </dd>
        </div>
        <div title={QUALITY_MEANINGS[it.quality as keyof typeof QUALITY_MEANINGS] ?? it.quality}>
          <dt>{TERMS.quality.name}</dt>
          <dd>{qualityLabel(it.quality)}</dd>
        </div>
        <div>
          <dt>네 칸 중</dt>
          <dd title={cellTitle(it.cell)}>{cellLabel(it.cell)}</dd>
        </div>
      </dl>

      {/* 두 글귀를 나란히 — 이 화면의 핵심이다 (§13 화면 6) */}
      <div className="detail-pair">
        <section className="glyph">
          <h3>감각 글귀</h3>
          <p className="glyph-prose">{detail.sensory_prose}</p>
          <Verdict reading={detail.sensory_reading} target={it.id} layer="sensory" onDone={reload} />
        </section>

        <section className="glyph">
          <h3>문화 글귀</h3>
          {detail.context_failed || !ctx ? (
            <div className="glyph-missing">
              <p>문화 글귀가 없습니다. 근거를 찾지 못했습니다.</p>
              <button type="button" className="btn" onClick={doRedo} disabled={pendingAction}>
                다시 시도
              </button>
            </div>
          ) : (
            <>
              {!ctx.grounded && <p className="glyph-warn">{NOT_GROUNDED_NOTICE}</p>}
              <p className="glyph-prose">{ctx.critique}</p>
              {ctx.lineage.length > 0 && <p className="glyph-lineage">{`계보: ${ctx.lineage.join(" · ")}`}</p>}
              <button type="button" className="btn btn-quiet" onClick={() => setShowSources((s) => !s)}>
                {`근거 ${ctx.sources.length}건 ${showSources ? "▴" : "▾"}`}
              </button>
              {showSources && (
                <ul className="glyph-sources">
                  {ctx.sources.map((s) => (
                    <li key={s.url}>
                      <a href={s.url} target="_blank" rel="noreferrer">
                        {s.title || s.url}
                      </a>
                    </li>
                  ))}
                </ul>
              )}
              {/* §12.6 — cultural reading 의 target 은 ingest 가 아니라 **context** 다. */}
              <Verdict reading={detail.cultural_reading} target={ctx.context_id} layer="cultural" onDone={reload} />
            </>
          )}
        </section>
      </div>

      {detail.can_recast && (
        <section className="detail-recast">
          {recastAsk === null ? (
            <button type="button" className="btn btn-quiet" onClick={askRecast}>
              {`${kindLabel(flipTo)}으로 다시 보기`}
            </button>
          ) : (
            <div className="confirm">
              <p>
                {`${kindLabel(flipTo)}으로 다시 읽으면 서술이 새로 만들어집니다. 여기에 이미 답한 ${recastAsk}건은 함께 넘어가지 않습니다.`}
              </p>
              <div className="confirm-btns">
                <button type="button" className="btn" onClick={doRecast} disabled={pendingAction}>
                  다시 읽는다
                </button>
                <button type="button" className="btn btn-quiet" onClick={() => setRecastAsk(null)}>
                  그만둔다
                </button>
              </div>
            </div>
          )}
        </section>
      )}

      <section className="detail-neighbors">
        <h3>이것과 비슷한 것</h3>
        {neighbors.length === 0 ? (
          <p className="facet-note">{EM_DASH}</p>
        ) : (
          <ul>
            {neighbors.map((n) => (
              <li key={n.id}>
                <button type="button" onClick={() => onSelect(n.id)} title={n.prose}>
                  {n.thumb_data_url ? <img src={n.thumb_data_url} alt="" /> : <span>{n.prose.slice(0, TILE_PROSE_CHARS)}</span>}
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}

// ────────────────────────────── ○/× — 버튼은 항상 정확히 둘이다 (§6.3 · T59)

function Verdict({
  reading,
  target,
  layer,
  onDone,
}: {
  reading: ReadingView | null;
  /** `sensory` 는 `ingest_id`, `cultural` 은 **`context_id`** 다 (§12.6). */
  target: string;
  layer: Layer;
  onDone: () => void;
}) {
  const [asking, setAsking] = useState(false);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);

  // 명세에 적힌 문구를 그대로 쓴다. 층마다 다르다 (§13 화면 2 · 2.1).
  const yesLabel = VERDICT_LABELS[layer].yes;
  const noLabel = VERDICT_LABELS[layer].no;

  if (reading) {
    const yes = reading.verdict === "yes";
    return (
      <div className="verdict is-done">
        <span className={yes ? "mark is-yes" : "mark is-no"}>{yes ? "○" : "×"}</span>
        <span className="verdict-label">{yes ? yesLabel : noLabel}</span>
        {reading.prose && <p className="verdict-prose">{reading.prose}</p>}
      </div>
    );
  }

  const send = async (verdict: "yes" | "no", prose: string | null) => {
    setBusy(true);
    try {
      await recordReading(target, layer, verdict, prose);
      onDone();
    } finally {
      setBusy(false);
    }
  };

  // 미응답 — 그 자리에서 답한다. 화면 2·2.1을 다시 띄우지 않는다 (T70).
  return (
    <div className="verdict">
      {asking ? (
        <div className="verdict-no">
          <input
            type="text"
            value={text}
            autoFocus
            placeholder="한 줄로 (건너뛰어도 됩니다)"
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void send("no", text.trim() === "" ? null : text.trim());
            }}
          />
          <button
            type="button"
            className="btn"
            disabled={busy}
            onClick={() => void send("no", text.trim() === "" ? null : text.trim())}
          >
            확인
          </button>
        </div>
      ) : (
        <div className="verdict-btns">
          <button type="button" className="btn btn-yes" disabled={busy} onClick={() => void send("yes", null)}>
            {yesLabel}
          </button>
          <button type="button" className="btn btn-no" disabled={busy} onClick={() => setAsking(true)}>
            {noLabel}
          </button>
        </div>
      )}
    </div>
  );
}
