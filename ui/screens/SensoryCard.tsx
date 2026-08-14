/**
 * 화면 2 — 감각 글귀 ○/× (§13 화면 2).
 *
 *   [썸네일]
 *   "차갑고 정돈된, 사람이 방금 지워진 실내"
 *
 *   [ 그렇다 ]        [ 아니다 ]
 *
 * - `그렇다` → 한 탭으로 끝. `verdict: yes`, `prose: null`
 * - `아니다` → 한 줄 입력창. 입력 없이 확인해도 `verdict: no`는 저장된다 (`prose: null`)
 * - 카드는 dismiss 할 수 있다. **무시하면 `reading` 관측을 만들지 않는다** — 호출하지 않으면 된다
 * - 하단에 "문화 글귀 준비 중"을 조용히 표시한다
 * - `kind_is_guess`(YouTube)면 한 번의 탭으로 kind를 뒤집는다. 이미 응답이 있으면
 *   `recast_preview`가 준 개수로 확인을 받는다 (§9.3)
 *
 * 버튼은 정확히 두 개다. 중간 선택지를 만들지 않는다 (§6.3 · §18-6 · T59).
 * 판단·계산은 전부 Rust에 있다. 이 파일은 뷰다 (§2).
 */

import { useEffect, useRef, useState } from "react";

import { errorText, recastKind, recastPreview, recordReading } from "../lib/api";
import type { Kind, SensoryCard as SensoryCardData, Verdict } from "../lib/types";
import { VERDICT_LABELS } from "../lib/types";
import "../styles/cards.css";

const LABEL = VERDICT_LABELS.sensory;

/** kind 표시명. 표시용 사전일 뿐 판정이 아니다. */
const KIND_LABEL: Record<string, string> = {
  text: "텍스트",
  image: "이미지",
  audio: "음악",
  video: "영상",
};

function kindLabel(kind: string): string {
  return KIND_LABEL[kind] ?? kind;
}

/** YouTube 추정은 audio ↔ video 둘뿐이다 (§9.3). */
function flipped(kind: string): Kind {
  return kind === "audio" ? "video" : "audio";
}

type Phase = "ask" | "correcting" | "saving" | "done";

export interface SensoryCardProps {
  card: SensoryCardData;
  /** `reading` 관측이 만들어진 뒤. 무시하고 넘긴 카드는 호출하지 않는다. */
  onAnswered?: (readingId: string, verdict: Verdict) => void;
  /** dismiss. 관측을 만들지 않는다. */
  onDismiss?: () => void;
  /** kind 뒤집기로 새 `ingest`가 생긴 뒤 (§9.3). */
  onRecast?: (next: SensoryCardData) => void;
  /** 하단의 "문화 글귀 준비 중". 아카이브 상세에서 다시 띄울 땐 끈다. */
  showContextPending?: boolean;
}

export function SensoryCard({
  card,
  onAnswered,
  onDismiss,
  onRecast,
  showContextPending = true,
}: SensoryCardProps) {
  const [current, setCurrent] = useState<SensoryCardData>(card);
  const [phase, setPhase] = useState<Phase>("ask");
  const [verdict, setVerdict] = useState<Verdict | null>(null);
  const [prose, setProse] = useState("");
  const [error, setError] = useState<string | null>(null);

  // kind 뒤집기 (§9.3)
  const [recasting, setRecasting] = useState(false);
  const [discardCount, setDiscardCount] = useState<number | null>(null);

  const inputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    setCurrent(card);
    setPhase("ask");
    setVerdict(null);
    setProse("");
    setError(null);
    setDiscardCount(null);
  }, [card]);

  useEffect(() => {
    if (phase === "correcting") inputRef.current?.focus();
  }, [phase]);

  async function answerYes() {
    if (phase === "saving" || phase === "done") return;
    setPhase("saving");
    setError(null);
    try {
      const id = await recordReading(current.ingest_id, "sensory", "yes", null);
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
    // 입력을 받은 뒤 확인에서 저장한다. 입력이 비어도 verdict는 저장된다.
    setPhase("correcting");
  }

  async function confirmNo() {
    if (phase === "saving" || phase === "done") return;
    setPhase("saving");
    setError(null);
    const text = prose.trim();
    try {
      const id = await recordReading(
        current.ingest_id,
        "sensory",
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

  /** 한 번의 탭. 버려질 응답이 있을 때만 확인을 받는다 (§9.3). */
  async function askRecast() {
    if (recasting) return;
    setRecasting(true);
    setError(null);
    try {
      const n = await recastPreview(current.ingest_id);
      if (n > 0) {
        setDiscardCount(n);
        setRecasting(false);
        return;
      }
      await doRecast();
    } catch (e) {
      setError(errorText(e));
      setRecasting(false);
    }
  }

  async function doRecast() {
    setRecasting(true);
    setError(null);
    try {
      const next = await recastKind(current.ingest_id, flipped(current.kind));
      setDiscardCount(null);
      // 이전 reading은 이월되지 않는다 (§9.3). 판정을 다시 받는다.
      setCurrent(next);
      setPhase("ask");
      setVerdict(null);
      setProse("");
      onRecast?.(next);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setRecasting(false);
    }
  }

  const busy = phase === "saving";
  const answered = phase === "done";

  return (
    <section className="ts-card" aria-label="감각 글귀">
      {onDismiss && !answered && (
        <button
          type="button"
          className="ts-card__dismiss"
          onClick={onDismiss}
          title="넘기기 — 답하지 않고 넘어갑니다. 아무것도 기록되지 않습니다."
          aria-label="넘기기"
        >
          ×
        </button>
      )}

      {current.thumb_data_url ? (
        <img className="ts-card__thumb" src={current.thumb_data_url} alt="" />
      ) : null}

      <div className="ts-card__kindrow">
        <span className="ts-chip">
          {current.kind_is_guess ? `추정: ${kindLabel(current.kind)}` : kindLabel(current.kind)}
        </span>
        {current.kind_is_guess && (
          <button
            type="button"
            className="ts-btn ts-btn--text"
            onClick={askRecast}
            disabled={recasting || busy}
          >
            {kindLabel(flipped(current.kind))}으로 보기
          </button>
        )}
      </div>

      {discardCount !== null && (
        <div className="ts-confirm" role="alertdialog" aria-label="kind 뒤집기 확인">
          <p className="ts-confirm__text">
            {kindLabel(flipped(current.kind))}으로 다시 읽으면 기계가 서술을 새로 씁니다. 여기에
            이미 답한 {discardCount}건은 함께 넘어가지 않습니다.
          </p>
          <div className="ts-confirm__row">
            <button
              type="button"
              className="ts-btn"
              onClick={doRecast}
              disabled={recasting}
            >
              다시 읽기
            </button>
            <button
              type="button"
              className="ts-btn"
              onClick={() => setDiscardCount(null)}
              disabled={recasting}
            >
              취소
            </button>
          </div>
        </div>
      )}

      <p className="ts-card__prose">{current.prose}</p>

      {/* ○/× 두 개. 중간 선택지를 두지 않는다 (T59). */}
      <div className="ts-verdicts" data-testid="sensory-verdicts">
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
              placeholder="그럼 무엇으로 보이는지 한 줄로 — 비워도 됩니다"
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

      {showContextPending && (
        <div className="ts-card__foot">
          <span>이것에 왜 끌렸을지도 곧 물어봅니다 · 준비 중</span>
        </div>
      )}
    </section>
  );
}

export default SensoryCard;
