/**
 * 화면 1 — 투입 (§13 화면 1).
 *
 * 드래그앤드롭 영역 + 전역 단축키 `Cmd/Ctrl+Shift+V`(클립보드 투입).
 *
 * Tauri v2 `onDragDropEvent`는 **파일 경로만** 준다. 브라우저에서 URL을 드래그하는 경로는
 * 이와 달라 동작하지 않으므로 시도하지 않는다. **YouTube 링크와 텍스트는 클립보드 단축키만이
 * 경로다.**
 *
 * 처리 중에는 모달리티별 단계를 표시한다 (예: 프레임 추출 → 오디오 변환 → 서술 → 병합).
 * YouTube 항목은 감각 카드까지 20~60초 걸린다 (§9.1 표). 기다리게 하되 무엇을 하는 중인지
 * 보여준다.
 *
 * 대기 중인 문화 글귀 건수를 조용히 표시한다 (§9.10). 건별 알림은 띄우지 않는다.
 *
 * kind 판별·거부 사유·단계 진행은 전부 Rust가 정한다. 이 파일은 뷰다 (§2).
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { readText } from "@tauri-apps/plugin-clipboard-manager";
import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import {
  isRegistered,
  register,
  unregister,
} from "@tauri-apps/plugin-global-shortcut";

import {
  critiquePendingCount,
  errorText,
  ingestClipboard,
  ingestFiles,
  setupStatus,
} from "../lib/api";
import type { SensoryCard as SensoryCardData } from "../lib/types";
import { EM_DASH } from "../lib/types";
import { CulturalCardDeck } from "./CulturalCard";
import { SensoryCard } from "./SensoryCard";
import "../styles/cards.css";

const SHORTCUT = "CommandOrControl+Shift+V";

/** 백엔드가 단계 진행을 알릴 때 쓰는 이벤트. 없으면 조용히 "처리 중"만 보인다. */
const PROGRESS_EVENT = "ingest://progress";

interface IngestProgressPayload {
  /** 파일 경로. 어떤 작업의 단계인지 맞추는 데 쓴다. */
  path?: string | null;
  /** 단계 키. 아래 표에 없으면 값을 그대로 보여준다. */
  stage?: string | null;
}

/** 단계 키 → 표시 문구. 표시용 사전일 뿐 순서를 정하지 않는다. */
const STAGE_LABEL: Record<string, string> = {
  probe: "판별",
  resolve: "링크 해석",
  metadata: "메타데이터",
  download: "내려받기",
  thumbnail: "썸네일",
  frames: "프레임 추출",
  audio: "오디오 변환",
  transcribe: "음성 서술",
  describe: "서술",
  merge: "병합",
  embed: "임베딩",
  record: "기록",
};

function stageLabel(stage: string): string {
  return STAGE_LABEL[stage] ?? stage;
}

function baseName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

function truncate(text: string, n: number): string {
  const flat = text.replace(/\s+/g, " ").trim();
  return flat.length > n ? `${flat.slice(0, n)}…` : flat;
}

function isMacLike(): boolean {
  return /Mac|iPhone|iPad/.test(navigator.userAgent);
}

let jobSeq = 0;

interface Job {
  id: string;
  label: string;
  /** 파일 투입이면 경로들. 진행 이벤트를 맞추는 데 쓴다. */
  paths: string[];
  startedAt: number;
  /** 백엔드가 알려준 단계만 순서대로 쌓인다. 프런트가 지어내지 않는다. */
  stages: string[];
  status: "running" | "done" | "error";
  error: string | null;
  cardCount: number;
}

export interface IngestProps {
  /**
   * 투입이 끝나 감각 카드가 생겼을 때. 넘기지 않으면 이 화면이 직접 카드를 띄운다.
   * 미응답 카드는 영속화하지 않는다 — 앱을 닫으면 사라진다 (§13 화면 2).
   */
  onCards?: (cards: SensoryCardData[]) => void;
  /**
   * 대기 중인 문화 글귀 목록을 열 때. 넘기지 않으면 이 화면이 직접 펼친다
   * — 셸에는 화면 2.1 탭이 없다 (§13 화면 2.1).
   */
  onOpenCulturalCards?: () => void;
  /** 투입 경로를 막을 때. 넘기지 않으면 `setup_status()`가 정한다 (§15). */
  disabled?: boolean;
  disabledReason?: string;
}

export function Ingest({
  onCards,
  onOpenCulturalCards,
  disabled,
  disabledReason,
}: IngestProps = {}) {
  const [dragOver, setDragOver] = useState(false);
  const [jobs, setJobs] = useState<Job[]>([]);
  const [cards, setCards] = useState<SensoryCardData[]>([]);
  const [pending, setPending] = useState<number | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  /** 경과 시간 표시용 현재 시각. 진행 중인 작업이 있을 때만 1초마다 갱신한다. */
  const [now, setNow] = useState(() => Date.now());
  const [deckOpen, setDeckOpen] = useState(false);
  /** §15 — 키·모델이 없으면 투입 경로가 전부 막힌다. 판정은 Rust가 한다. */
  const [blocked, setBlocked] = useState<string | null>(null);
  /**
   * §9.10 · T54 — `false`면 문화 층 전체가 없다. 대기 건수도 "준비 중"도 띄우지 않는다.
   * 아직 못 읽었으면 `null`이며, 기본값(`true`)처럼 다룬다.
   */
  const [contextEnabled, setContextEnabled] = useState<boolean | null>(null);
  const showCultural = contextEnabled !== false;

  const blockedByProps = disabled === true;
  const isBlocked = disabled ?? blocked !== null;
  const blockReason =
    disabledReason ?? (blockedByProps ? null : blocked) ?? "설정에서 API 키를 넣으면 열립니다.";

  const disabledRef = useRef(isBlocked);
  disabledRef.current = isBlocked;

  const emitCards = useCallback(
    (next: SensoryCardData[]) => {
      if (next.length === 0) return;
      if (onCards) onCards(next);
      else setCards((prev) => [...next, ...prev]);
    },
    [onCards],
  );

  const refreshPending = useCallback(async () => {
    try {
      setPending(await critiquePendingCount());
    } catch {
      // 대기 건수는 조용한 표시다. 실패하면 그냥 비운다.
      setPending(null);
    }
  }, []);

  const startJob = useCallback((label: string, paths: string[]): Job => {
    const job: Job = {
      id: `job-${++jobSeq}`,
      label,
      paths,
      startedAt: Date.now(),
      stages: [],
      status: "running",
      error: null,
      cardCount: 0,
    };
    setJobs((prev) => [job, ...prev]);
    return job;
  }, []);

  const finishJob = useCallback(
    (id: string, patch: Partial<Job>) => {
      setJobs((prev) => prev.map((j) => (j.id === id ? { ...j, ...patch } : j)));
    },
    [],
  );

  /** 파일 투입 — 드롭과 파일 선택이 같은 경로를 탄다. */
  const submitFiles = useCallback(
    async (paths: string[]) => {
      if (disabledRef.current || paths.length === 0) return;
      setNotice(null);
      const label =
        paths.length === 1
          ? baseName(paths[0])
          : `${baseName(paths[0])} 외 ${paths.length - 1}개`;
      const job = startJob(label, paths);
      try {
        const result = await ingestFiles(paths);
        finishJob(job.id, { status: "done", cardCount: result.length });
        emitCards(result);
        void refreshPending();
      } catch (e) {
        finishJob(job.id, { status: "error", error: errorText(e) });
      }
    },
    [emitCards, finishJob, refreshPending, startJob],
  );

  /** 클립보드 투입 — YouTube 링크와 텍스트의 유일한 경로다. */
  const submitClipboard = useCallback(async () => {
    if (disabledRef.current) return;
    setNotice(null);
    let text: string;
    try {
      text = await readText();
    } catch (e) {
      setNotice(`클립보드를 읽지 못했습니다 — ${errorText(e)}`);
      return;
    }
    if (!text || text.trim().length === 0) {
      setNotice("클립보드가 비어 있습니다.");
      return;
    }
    const job = startJob(truncate(text, 60), []);
    try {
      const card = await ingestClipboard(text);
      finishJob(job.id, { status: "done", cardCount: 1 });
      emitCards([card]);
      void refreshPending();
    } catch (e) {
      // kind 판별과 거부 사유는 Rust가 정한다 (§9.1). 문구를 그대로 보여준다.
      finishJob(job.id, { status: "error", error: errorText(e) });
    }
  }, [emitCards, finishJob, refreshPending, startJob]);

  // 전역 단축키는 최신 핸들러를 봐야 한다.
  const clipboardRef = useRef(submitClipboard);
  clipboardRef.current = submitClipboard;

  // ─── 드래그앤드롭. 경로만 온다 (§13 화면 1).
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let disposed = false;
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        const p = event.payload;
        if (p.type === "enter" || p.type === "over") setDragOver(true);
        else if (p.type === "leave") setDragOver(false);
        else if (p.type === "drop") {
          setDragOver(false);
          void submitFiles(p.paths);
        }
      })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {
        /* 드롭 리스너를 못 붙이면 단축키와 파일 선택이 남는다. */
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [submitFiles]);

  // ─── 전역 단축키 Cmd/Ctrl+Shift+V (§13 화면 1).
  useEffect(() => {
    let disposed = false;
    void (async () => {
      try {
        if (await isRegistered(SHORTCUT)) await unregister(SHORTCUT);
        if (disposed) return;
        await register(SHORTCUT, (event) => {
          if (event.state === "Pressed") void clipboardRef.current();
        });
      } catch {
        /* 다른 앱이 쥐고 있으면 버튼으로 붙여넣는다. */
      }
    })();
    return () => {
      disposed = true;
      void unregister(SHORTCUT).catch(() => {});
    };
  }, []);

  // ─── 단계 진행. 백엔드가 알려주는 만큼만 보여준다.
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let disposed = false;
    void listen<IngestProgressPayload>(PROGRESS_EVENT, (event) => {
      const stage = event.payload?.stage;
      if (!stage) return;
      const path = event.payload?.path ?? null;
      setJobs((prev) => {
        const running = prev.filter((j) => j.status === "running");
        const target = path
          ? running.find((j) => j.paths.includes(path))
          : running.length === 1
            ? running[0]
            : undefined;
        // 어느 작업인지 모르면 아무것도 그리지 않는다. 지어내지 않는다.
        if (!target) return prev;
        return prev.map((j) =>
          j.id === target.id && j.stages[j.stages.length - 1] !== stage
            ? { ...j, stages: [...j.stages, stage] }
            : j,
        );
      });
    })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {});
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  // ─── §15 — 키·모델이 없으면 투입 경로가 막힌다. 판정 자체는 Rust가 준다.
  useEffect(() => {
    let alive = true;
    void setupStatus()
      .then((s) => {
        if (!alive) return;
        setContextEnabled(s.context_enabled);
        if (disabled !== undefined) return;
        if (!s.api_key_set) setBlocked("API 키가 없습니다. 설정에서 넣으면 열립니다.");
        else if (s.models_unset) setBlocked("모델이 설정되지 않았습니다. 설정에서 진단을 실행하세요.");
        else setBlocked(null);
      })
      .catch(() => {
        /* 상태를 못 읽으면 막지 않는다. 실패는 투입 시점에 그대로 보인다. */
      });
    return () => {
      alive = false;
    };
  }, [disabled]);

  // ─── 대기 건수 (§9.10). 조용히, 건별 알림 없이. 문화 층이 꺼져 있으면 묻지 않는다 (T54).
  useEffect(() => {
    if (!showCultural) return;
    void refreshPending();
    const t = window.setInterval(() => void refreshPending(), 15000);
    return () => window.clearInterval(t);
  }, [refreshPending, showCultural]);

  // ─── 경과 시간 표시. 진행 중인 작업이 있을 때만 돈다.
  const running = jobs.some((j) => j.status === "running");
  useEffect(() => {
    if (!running) return;
    setNow(Date.now());
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, [running]);

  async function pickFiles() {
    if (isBlocked) return;
    try {
      const picked = await openFileDialog({ multiple: true, title: "투입할 파일" });
      if (!picked) return;
      await submitFiles(Array.isArray(picked) ? picked : [picked]);
    } catch (e) {
      setNotice(errorText(e));
    }
  }

  const shortcutLabel = isMacLike() ? "⌘⇧V" : "Ctrl+Shift+V";

  return (
    <div className="ts-screen">
      <div className="ts-wrap">
        <h1 className="ts-h">투입</h1>
        <p className="ts-sub">넣은 것에 대해 두 번 묻는다. 답은 두 개뿐이다.</p>

        <div
          className={
            "ts-drop" +
            (dragOver ? " ts-drop--over" : "") +
            (isBlocked ? " ts-drop--off" : "")
          }
          aria-label="투입 영역"
        >
          <p className="ts-drop__title">
            {isBlocked ? "투입이 막혀 있습니다" : "이미지 · 영상 파일을 여기에 놓으세요"}
          </p>
          <p className="ts-drop__hint">
            {isBlocked ? (
              blockReason
            ) : (
              <>
                YouTube 링크와 텍스트는 <span className="ts-kbd">{shortcutLabel}</span> 로
                붙여넣습니다. 링크를 끌어다 놓는 것은 동작하지 않습니다.
              </>
            )}
          </p>
          {!isBlocked && (
            <div className="ts-drop__actions">
              <button type="button" className="ts-btn" onClick={pickFiles}>
                파일 고르기
              </button>
              <button
                type="button"
                className="ts-btn"
                onClick={() => void submitClipboard()}
              >
                클립보드에서 붙여넣기
              </button>
            </div>
          )}
        </div>

        {notice && <p className="ts-error-text">{notice}</p>}

        {jobs.length > 0 && (
          <ul className="ts-jobs">
            {jobs.map((job) => (
              <li className="ts-job" key={job.id}>
                <div className="ts-job__head">
                  <span className="ts-job__label">{job.label}</span>
                  <span className="ts-job__elapsed">
                    {job.status === "running"
                      ? `${Math.max(0, Math.floor((now - job.startedAt) / 1000))}초`
                      : job.status === "done"
                        ? `카드 ${job.cardCount}장`
                        : "실패"}
                  </span>
                </div>

                {job.status === "running" && (
                  <>
                    <div className="ts-stages">
                      {job.stages.length === 0 ? (
                        <span className="ts-stage ts-stage--active ts-dots">처리 중</span>
                      ) : (
                        job.stages.map((s, i) => (
                          <span key={`${s}-${i}`}>
                            {i > 0 && <span className="ts-stage__sep"> → </span>}
                            <span
                              className={
                                "ts-stage " +
                                (i === job.stages.length - 1
                                  ? "ts-stage--active ts-dots"
                                  : "ts-stage--done")
                              }
                            >
                              {stageLabel(s)}
                            </span>
                          </span>
                        ))
                      )}
                    </div>
                    {now - job.startedAt > 8000 && (
                      <p className="ts-job__wait">
                        내려받기가 필요한 항목은 20~60초 걸립니다. 그대로 두면 됩니다.
                      </p>
                    )}
                  </>
                )}

                {job.status === "error" && job.error && (
                  <p className="ts-error-text">{job.error}</p>
                )}
              </li>
            ))}
          </ul>
        )}

        {/* §9.10 — 대기 건수는 조용히. 건별 알림을 띄우지 않는다 (T51e). */}
        {showCultural && (
          <div className="ts-pending">
            <span>
              문화 글귀 대기{" "}
              {pending === null ? <span className="ts-null">{EM_DASH}</span> : `${pending}건`}
            </span>
            <button
              type="button"
              className="ts-btn ts-btn--text"
              onClick={() => {
                if (onOpenCulturalCards) onOpenCulturalCards();
                else setDeckOpen((v) => !v);
              }}
            >
              {onOpenCulturalCards ? "목록 열기" : deckOpen ? "목록 닫기" : "목록 열기"}
            </button>
          </div>
        )}

        {/* 셸에 화면 2.1 탭이 없다. 목록은 여기서 연다 (§13 화면 2.1). */}
        {showCultural && deckOpen && !onOpenCulturalCards && (
          <CulturalCardDeck onChange={() => void refreshPending()} />
        )}

        {/* onCards를 받지 않으면 이 화면이 감각 카드를 직접 띄운다 (§9.1 → 화면 2). */}
        {cards.map((card) => (
          <SensoryCard
            key={card.ingest_id}
            card={card}
            showContextPending={showCultural}
            onDismiss={() =>
              setCards((prev) => prev.filter((c) => c.ingest_id !== card.ingest_id))
            }
            onRecast={(next) =>
              setCards((prev) =>
                prev.map((c) => (c.ingest_id === card.ingest_id ? next : c)),
              )
            }
          />
        ))}
      </div>
    </div>
  );
}

export default Ingest;
