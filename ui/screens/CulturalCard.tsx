/**
 * 화면 2.1 — 문화 글귀 ○/× (§13 화면 2.1).
 *
 *   "잔향을 악기처럼 다루던 90년대 초 영국 기타 음악의 어법을 따르되, …"
 *
 *   계보: 슈게이즈 · 드림 팝            근거 3건 ▾
 *
 *   [ 그래서 좋다 ]        [ 그것 때문은 아니다 ]
 *
 * - `그래서 좋다` → `verdict: yes`, `prose: null`
 * - `그것 때문은 아니다` → 한 줄 입력창. 비워도 `verdict: no`는 저장된다 (`prose: null`)
 * - `근거 N건`을 펼치면 `sources`의 제목과 URL이 보인다. 사용자가 직접 확인할 수 있어야 한다
 * - `grounded === false`면 카드 상단에 `NOT_GROUNDED_NOTICE` (T58)
 * - 화면 2와 달리 **미응답 시에도 유지된다.** 대기 목록을 한 번에 넘겨보는 뷰가 이 화면이다 (T53)
 *
 * 버튼은 정확히 두 개다. 중간 선택지를 만들지 않는다 (§6.3 · §18-6 · T59).
 * 판단·계산은 전부 Rust에 있다. 이 파일은 뷰다 (§2).
 *
 * ## export
 * - `CulturalCard` (기본 export) — 화면 2.1 그 자체. props 없이 대기 목록을 스스로 읽는다
 * - `CulturalCardScreen` — 같은 것의 별칭
 * - `CulturalCardDeck` — 화면 껍데기 없는 대기 목록. 화면 1이 이것을 펼쳐 연다
 * - `CulturalCardView` — 카드 한 장. 화면 6 항목 상세에서 재사용할 수 있다
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";

import { errorText, pendingCulturalCards, recordReading } from "../lib/api";
import type { CulturalCard as CulturalCardData, Verdict } from "../lib/types";
import { EM_DASH, NOT_GROUNDED_NOTICE, VERDICT_LABELS } from "../lib/types";
import "../styles/cards.css";

const LABEL = VERDICT_LABELS.cultural;

type Phase = "ask" | "correcting" | "saving" | "done";

export interface CulturalCardViewProps {
  card: CulturalCardData;
  /** `reading` 관측이 만들어진 뒤. 답하지 않고 넘긴 카드는 호출하지 않는다. */
  onAnswered?: (readingId: string, verdict: Verdict) => void;
}

/** 카드 한 장. */
export function CulturalCardView({ card, onAnswered }: CulturalCardViewProps) {
  const [phase, setPhase] = useState<Phase>("ask");
  const [verdict, setVerdict] = useState<Verdict | null>(null);
  const [prose, setProse] = useState("");
  const [sourcesOpen, setSourcesOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const inputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    setPhase("ask");
    setVerdict(null);
    setProse("");
    setSourcesOpen(false);
    setError(null);
  }, [card]);

  useEffect(() => {
    if (phase === "correcting") inputRef.current?.focus();
  }, [phase]);

  async function answerYes() {
    if (phase === "saving" || phase === "done") return;
    setPhase("saving");
    setError(null);
    try {
      // cultural 층의 target은 `ingest`가 아니라 `context` ID다 (§6.3).
      const id = await recordReading(card.context_id, "cultural", "yes", null);
      setVerdict("yes");
      setPhase("done");
      onAnswered?.(id, "yes");
    } catch (e) {
      setError(errorText(e));
      setPhase("ask");
    }
  }

  function answerNo() {
    if (phase === "saving" || phase === "done") return;
    setError(null);
    setPhase("correcting");
  }

  async function confirmNo() {
    if (phase === "saving" || phase === "done") return;
    setPhase("saving");
    setError(null);
    const text = prose.trim();
    try {
      const id = await recordReading(
        card.context_id,
        "cultural",
        "no",
        text.length > 0 ? text : null,
      );
      setVerdict("no");
      setPhase("done");
      onAnswered?.(id, "no");
    } catch (e) {
      setError(errorText(e));
      setPhase("correcting");
    }
  }

  const busy = phase === "saving";
  const answered = phase === "done";

  return (
    <section className="ts-card" aria-label="문화 글귀">
      {/* T58 — 근거가 얇으면 그렇다고 먼저 말한다. */}
      {!card.grounded && <p className="ts-card__banner">{NOT_GROUNDED_NOTICE}</p>}

      {card.thumb_data_url ? (
        <img className="ts-card__thumb" src={card.thumb_data_url} alt="" />
      ) : null}

      <p className="ts-card__prose ts-card__critique">{card.critique}</p>

      <p className="ts-card__lineage">
        계보:{" "}
        {card.lineage.length > 0 ? (
          card.lineage.join(" · ")
        ) : (
          /* §R10 — 없는 값은 em dash 하나로 렌더한다. */
          <span className="ts-null">{EM_DASH}</span>
        )}
      </p>

      <div className="ts-sources">
        {card.sources.length > 0 ? (
          <>
            <button
              type="button"
              className="ts-sources__toggle"
              aria-expanded={sourcesOpen}
              onClick={() => setSourcesOpen((v) => !v)}
            >
              근거 {card.sources.length}건 {sourcesOpen ? "▴" : "▾"}
            </button>
            {sourcesOpen && (
              <ul className="ts-sources__list">
                {card.sources.map((s, i) => (
                  <li className="ts-sources__item" key={`${s.url}-${i}`}>
                    <span className="ts-sources__title">{s.title}</span>
                    <button
                      type="button"
                      className="ts-sources__url"
                      title={s.url}
                      onClick={() => {
                        void openUrl(s.url).catch((e: unknown) => setError(errorText(e)));
                      }}
                    >
                      {s.url}
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </>
        ) : (
          <span className="ts-muted">근거 0건</span>
        )}
      </div>

      {/* ○/× 두 개. 중간 선택지를 두지 않는다 (T59). */}
      <div className="ts-verdicts" data-testid="cultural-verdicts">
        <button
          type="button"
          className="ts-verdict"
          data-verdict="yes"
          aria-pressed={verdict === "yes"}
          onClick={answerYes}
          disabled={busy || answered}
        >
          {LABEL.yes}
        </button>
        <button
          type="button"
          className="ts-verdict"
          data-verdict="no"
          aria-pressed={verdict === "no" || phase === "correcting"}
          onClick={answerNo}
          disabled={busy || answered}
        >
          {LABEL.no}
        </button>
      </div>

      {phase === "correcting" && (
        <>
          <div className="ts-correct">
            <input
              ref={inputRef}
              className="ts-input"
              type="text"
              value={prose}
              placeholder="그럼 무엇 때문인지 한 줄로 (비워도 됩니다)"
              onChange={(e) => setProse(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") void confirmNo();
              }}
            />
            <button type="button" className="ts-btn" onClick={confirmNo}>
              확인
            </button>
          </div>
          <p className="ts-hint">비워두고 확인해도 '{LABEL.no}'는 기록됩니다.</p>
        </>
      )}

      {answered && (
        <p className="ts-card__done">
          기록했습니다 — {verdict === "yes" ? LABEL.yes : LABEL.no}
        </p>
      )}

      {error && <p className="ts-error-text">{error}</p>}
    </section>
  );
}

/* ───────────────────────────────────────────────── 대기 목록 (§13 화면 2.1)
 *
 * 나중에 도착하는 카드라 그 순간 사용자가 화면을 보고 있으리란 보장이 없다.
 * 앱을 다시 열면 여기에 남아 있다 (T53). 한 번에 넘겨본다.
 */

export interface CulturalCardDeckProps {
  /** 목록이 줄었을 때. 바깥의 대기 건수 표시를 갱신하라. */
  onChange?: () => void;
}

/** 대기 목록. 화면 껍데기가 없어 화면 1이 그대로 펼쳐 쓸 수 있다. */
export function CulturalCardDeck({ onChange }: CulturalCardDeckProps = {}) {
  const [cards, setCards] = useState<CulturalCardData[]>([]);
  const [index, setIndex] = useState(0);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const next = await pendingCulturalCards();
      setCards(next);
      setIndex((i) => (next.length === 0 ? 0 : Math.min(i, next.length - 1)));
    } catch (e) {
      setError(errorText(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  /** 답한 카드만 목록에서 뺀다. 넘긴 카드는 그대로 남는다 (T53). */
  function handleAnswered() {
    const answeredId = cards[index]?.context_id;
    const remaining = cards.length - 1;
    setCards((prev) => prev.filter((c) => c.context_id !== answeredId));
    setIndex((i) => Math.max(0, Math.min(i, remaining - 1)));
    onChange?.();
  }

  const current = cards[index];

  return (
    <>
      <div className="ts-deck__head">
        <span className="ts-muted">
          {loading ? "불러오는 중" : `대기 ${cards.length}건`}
        </span>
        <div className="ts-deck__nav">
          <button
            type="button"
            className="ts-deck__arrow"
            onClick={() => setIndex((i) => Math.max(0, i - 1))}
            disabled={index <= 0}
            aria-label="이전 카드"
          >
            ‹
          </button>
          <span>{cards.length === 0 ? EM_DASH : `${index + 1} / ${cards.length}`}</span>
          <button
            type="button"
            className="ts-deck__arrow"
            onClick={() => setIndex((i) => Math.min(cards.length - 1, i + 1))}
            disabled={index >= cards.length - 1}
            aria-label="다음 카드"
          >
            ›
          </button>
          <button type="button" className="ts-btn ts-btn--text" onClick={() => void load()}>
            새로 고침
          </button>
        </div>
      </div>

      {error && <p className="ts-error-text">{error}</p>}

      {current ? (
        <CulturalCardView
          key={current.context_id}
          card={current}
          onAnswered={handleAnswered}
        />
      ) : (
        !loading && (
          <p className="ts-empty">
            대기 중인 문화 글귀가 없습니다. 투입하면 20~60초 뒤에 도착합니다.
          </p>
        )
      )}
    </>
  );
}

/** 화면 2.1. props 없이 스스로 대기 목록을 읽는다. */
export function CulturalCard() {
  return (
    <div className="ts-screen">
      <div className="ts-wrap">
        <h1 className="ts-h">문화 글귀</h1>
        <p className="ts-sub">이것 때문에 좋은지 묻는다. 답하지 않은 카드는 여기 남는다.</p>
        <CulturalCardDeck />
      </div>
    </div>
  );
}

/** 화면 이름으로 부르고 싶을 때. `CulturalCard`와 같은 것이다. */
export const CulturalCardScreen = CulturalCard;

export default CulturalCard;
