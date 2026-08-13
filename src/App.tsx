/**
 * 앱 셸 — 탭 전환과 §D7 고지 게이트만 맡는다.
 *
 * **프런트는 순수 뷰다** (§2). 여기에는 판단도 계산도 없다.
 * 각 화면이 `lib/api.ts` 로 자기 데이터를 직접 가져온다.
 * 셸이 부르는 커맨드는 `setup_status()` 하나뿐이다.
 *
 * 라우터를 쓰지 않는다. 탭은 state 하나다.
 */

import { useCallback, useEffect, useState } from "react";
import { errorText, setupStatus } from "./lib/api";
import type { SetupStatus } from "./lib/types";

import BoundaryNotice from "./components/BoundaryNotice";
import ErrorBoundary from "./components/ErrorBoundary";
import Archive from "./screens/Archive";
import ApproveDiff from "./screens/ApproveDiff";
import CulturalCardList from "./screens/CulturalCard";
import Dashboard from "./screens/Dashboard";
import Ingest from "./screens/Ingest";
import Setup from "./screens/Setup";
import SoulDoc from "./screens/SoulDoc";

type TabId = "ingest" | "soul" | "dashboard" | "archive" | "settings";

const TABS: { id: TabId; label: string }[] = [
  { id: "ingest", label: "투입" },
  { id: "soul", label: "SOUL" },
  { id: "dashboard", label: "대시보드" },
  { id: "archive", label: "아카이브" },
  { id: "settings", label: "설정" },
];

export default function App() {
  const [status, setStatus] = useState<SetupStatus | null>(null);
  const [bootError, setBootError] = useState<string | null>(null);
  const [tab, setTab] = useState<TabId>("ingest");
  const [culturalOpen, setCulturalOpen] = useState(false);
  /** 대시보드에서 셀을 눌러 아카이브로 넘어올 때의 초기 패싯 (§13 화면 6). */
  const [archiveCell, setArchiveCell] = useState<string | null>(null);

  const loadStatus = useCallback(async () => {
    try {
      const s = await setupStatus();
      setStatus(s);
      setBootError(null);
      // §15 — 키·모델이 없으면 모든 투입 경로가 막힌다. 설정 화면으로 유도한다.
      if (!s.needs_boundary_notice && (!s.api_key_set || s.models_unset)) {
        setTab("settings");
      }
    } catch (e) {
      setStatus(null);
      setBootError(errorText(e));
    }
  }, []);

  useEffect(() => {
    void loadStatus();
  }, [loadStatus]);

  const ready = status !== null && !status.needs_boundary_notice;

  // Cmd/Ctrl + 1‥5 로 탭 전환.
  useEffect(() => {
    if (!ready) return;
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey) || e.altKey || e.shiftKey) return;
      const i = Number(e.key) - 1;
      if (!Number.isInteger(i) || i < 0 || i >= TABS.length) return;
      const el = e.target as HTMLElement | null;
      if (el && (el.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName))) return;
      e.preventDefault();
      setTab(TABS[i].id);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [ready]);

  // 대기 목록 겹침은 Esc 로 닫는다.
  useEffect(() => {
    if (!culturalOpen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setCulturalOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [culturalOpen]);

  if (bootError !== null) {
    return (
      <div className="app">
        <div className="app__boot">
          <p className="warn-text">앱 상태를 읽지 못했습니다</p>
          <p className="small mute selectable">{bootError}</p>
          <button type="button" className="btn" onClick={() => void loadStatus()}>
            다시 시도
          </button>
        </div>
      </div>
    );
  }

  if (status === null) {
    return (
      <div className="app">
        <div className="app__boot">
          <span className="spinner" aria-hidden />
          <span className="small">준비 중</span>
        </div>
      </div>
    );
  }

  // §D7 — 첫 투입 전에 §D2 표를 그대로 보여주고 확인을 받는다.
  // `acknowledge_boundary(context_enabled)` 는 고지 화면이 직접 부른다.
  if (status.needs_boundary_notice) {
    return (
      <div className="app">
        <main className="app__main">
          <BoundaryNotice onDone={() => void loadStatus()} />
        </main>
      </div>
    );
  }

  // §15 — 키 미설정이면 투입 경로를 막고 설정으로 유도한다.
  const blocked = !status.api_key_set || status.models_unset;
  const blockedReason = !status.api_key_set
    ? "API 키가 설정되지 않았습니다. 설정에서 등록하세요."
    : "모델이 설정되지 않았습니다. 설정에서 진단을 실행하세요.";

  return (
    <div className="app">
      <nav className="app__tabs" aria-label="화면">
        {TABS.map((t) => (
          <button
            key={t.id}
            type="button"
            className={t.id === tab ? "tab tab--on" : "tab"}
            aria-current={t.id === tab ? "page" : undefined}
            onClick={() => setTab(t.id)}
          >
            {t.label}
          </button>
        ))}
        <span className="spacer" />
      </nav>

      {blocked && (
        <div className="app__banner" role="status">
          <span>{blockedReason}</span>
          <span className="spacer" />
          {tab !== "settings" && (
            <button
              type="button"
              className="btn btn--sm btn--ghost"
              onClick={() => setTab("settings")}
            >
              설정 열기
            </button>
          )}
        </div>
      )}

      <main className="app__main">
        {/*
          화면 1. 문화 글귀 대기 건수는 이 화면이 하단에 조용히 표시한다 (§9.10 · §13 화면 2.1).
          셸은 같은 수를 두 번 그리지 않고, 목록을 여는 경로만 넘긴다 (T51e).
        */}
        {tab === "ingest" && (
          <ErrorBoundary screen="투입">
            <Ingest
              onOpenCulturalCards={() => setCulturalOpen(true)}
              disabled={blocked}
              disabledReason={blocked ? blockedReason : undefined}
            />
          </ErrorBoundary>
        )}

        {/* 화면 3 + 화면 4 — 성찰 제안은 SOUL.md 편집과 같은 자리에서 승인한다. */}
        {tab === "soul" && (
          <ErrorBoundary screen="SOUL">
            <div className="stack-lg">
              <SoulDoc />
              <ApproveDiff />
            </div>
          </ErrorBoundary>
        )}

        {/* 화면 5 → 6 — "대시보드에서 넘어가는 공간"이다. 누른 셀을 초기 패싯으로 넘긴다. */}
        {tab === "dashboard" && (
          <ErrorBoundary screen="대시보드">
            <Dashboard
              onOpenArchive={(cell) => {
                setArchiveCell(cell ?? null);
                setTab("archive");
              }}
            />
          </ErrorBoundary>
        )}
        {tab === "archive" && (
          <ErrorBoundary screen="아카이브">
            <Archive
              key={archiveCell ?? "all"}
              initialCell={archiveCell ?? undefined}
            />
          </ErrorBoundary>
        )}
        {/*
          설정은 특히 감싼다. 여기가 터지면 키·모델을 고칠 길이 사라지고,
          그것은 다른 모든 화면이 막힌 상태의 **유일한 탈출구**다 (§15).
        */}
        {tab === "settings" && (
          <ErrorBoundary screen="설정">
            <Setup />
          </ErrorBoundary>
        )}
      </main>

      {/* 화면 2.1 — 대기 목록은 한 번에 넘겨본다. 건별 알림을 띄우지 않는다 (T51e). */}
      {culturalOpen && (
        <div className="overlay" role="dialog" aria-modal="true" aria-label="문화 글귀 대기 목록">
          <div className="overlay__panel">
            <div className="row">
              <span className="spacer" />
              <button
                type="button"
                className="btn btn--sm btn--ghost"
                onClick={() => setCulturalOpen(false)}
              >
                닫기
              </button>
            </div>
            <CulturalCardList />
          </div>
        </div>
      )}
    </div>
  );
}
