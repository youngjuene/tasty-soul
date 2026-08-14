/**
 * §D7 — 최초 실행 고지.
 *
 * `show_boundary_on_first_run`이 참이면 `soul doctor` 직후, 첫 투입 전에
 * **§D2 표를 그대로** 보여주고 확인을 받는다. 축약하거나 "로컬 우선" 같은 문구로
 * 뭉뚱그리지 않는다.
 *
 * 이때 `context_enabled` 선택을 함께 받는다 — "투입할 때마다 웹 검색이 일어납니다"를
 * 명시하고 끄고 시작할 수 있게 한다. 기본값은 켬이되 선택 자체를 숨기지 않는다.
 *
 * 셸(`App.tsx`)이 이 화면을 게이트로 세운다. 확인이 끝나면 `onDone`으로 알린다.
 */
import { useState } from "react";
import { acknowledgeBoundary, errorText } from "../lib/api";
import "../styles/doc.css";

export interface BoundaryNoticeProps {
  /** §D7 — 기본값은 켬이다. `setup_status().context_enabled`를 그대로 넘겨도 된다 */
  defaultContextEnabled?: boolean;
  /** 확인이 끝난 뒤. 셸이 `setup_status()`를 다시 읽는다 */
  onAcknowledged?: (contextEnabled: boolean) => void;
  /** `lib/types.ts`가 쓰는 이름. 둘 중 아무거나 넘겨도 된다 */
  onDone?: () => void;
}

/** `전송` · `노출` · `전송하지 않음` — 색으로만 갈래를 나누고 문구는 §D2 그대로 둔다 */
type Tone = "sent" | "expose" | "kept";

interface BoundaryRow {
  data: string;
  sent: string;
  tone: Tone;
  /** §D2에서 굵게 쓴 칸 */
  strongData?: boolean;
  strongSent?: boolean;
}

/** §D2. 행을 빼거나 합치지 않는다. */
const D2_ROWS: BoundaryRow[] = [
  { data: "이미지 (긴 변 1280 JPEG, EXIF 제거)", sent: "전송", tone: "sent" },
  {
    data: "오디오 (YouTube에서 받은 최대 30초 샘플) + 제목·채널·설명",
    sent: "전송",
    tone: "sent",
  },
  {
    data: "영상 앞 30초의 1fps 프레임 (최대 30장) + 같은 구간 오디오",
    sent: "전송",
    tone: "sent",
  },
  { data: "붙여넣은 텍스트 (최대 4000자)", sent: "전송", tone: "sent" },
  {
    data: "YouTube video id·oEmbed·Data API 호출",
    sent: "YouTube 서버에 IP 노출 (§D6)",
    tone: "expose",
  },
  {
    data: "문화 글귀 생성 시 검색어",
    sent: "투입마다 자동으로 검색 제공자에게 전송 (§D6)",
    tone: "expose",
    strongData: true,
    strongSent: true,
  },
  {
    data: "문화 글귀 생성 시 방문한 페이지",
    sent: "해당 서버에 IP 노출 (§D6)",
    tone: "expose",
  },
  { data: "`machine.prose`", sent: "임베딩 엔드포인트로 전송", tone: "sent" },
  {
    data: "`reading.prose` (사용자 본인의 문장)",
    sent: "임베딩 엔드포인트로 전송",
    tone: "sent",
  },
  {
    data: "`SOUL.md`의 `soul:gen` · `soul:neg` 블록",
    sent: "성찰 호출 시 전송",
    tone: "sent",
  },
  {
    data: "`SOUL.md`의 `soul:human` 블록",
    sent: "원격으로 전송하지 않는다. 로컬 내보내기에는 포함된다 (§D4)",
    tone: "kept",
    strongData: true,
    strongSent: true,
  },
  {
    data: "`soul-mcp` 서버가 다루는 모든 데이터",
    sent: "전송하지 않음. 서버에 네트워크 클라이언트를 링크하지 않는다 (§19.4)",
    tone: "kept",
  },
  {
    data: "원본 파일 자체",
    sent: "전송하지 않음. 앱이 복사하지도 않는다 (§20.4)",
    tone: "kept",
  },
  { data: "임베딩 벡터", sent: "전송하지 않음", tone: "kept" },
  { data: "git 이력 · `runs/` 트레이스", sent: "전송하지 않음", tone: "kept" },
];

/** 백틱 구간만 `code`로 감싼다. 그 밖의 인라인 문법은 쓰지 않는다 */
function Ticks({ text }: { text: string }) {
  const parts = text.split("`");
  return (
    <>
      {parts.map((p, i) =>
        i % 2 === 1 ? (
          <code key={i} className="doc-code">
            {p}
          </code>
        ) : (
          <span key={i}>{p}</span>
        ),
      )}
    </>
  );
}

export function BoundaryNotice({
  defaultContextEnabled = true,
  onAcknowledged,
  onDone,
}: BoundaryNoticeProps) {
  // §D7 — 기본값은 켬이다. 선택 자체를 숨기지 않는다.
  const [contextEnabled, setContextEnabled] = useState(defaultContextEnabled);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function confirm() {
    setBusy(true);
    setError(null);
    try {
      await acknowledgeBoundary(contextEnabled);
      onAcknowledged?.(contextEnabled);
      onDone?.();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="doc doc-boundary" aria-labelledby="bn-title">
      <h1 id="bn-title" className="doc-title">
        무엇이 이 기기를 떠나는가
      </h1>

      <p className="doc-lead">
        취향 기록은 개인적인 자료다. 이 앱은 두 국면으로 나뉘고 프라이버시 성질이 서로 다르다.
        <strong> 구축</strong>(미디어 해석 · 임베딩 · 성찰)은 OpenAI API로 전송된다. 불가피하다.
        <strong> 적용</strong>(로컬 에이전트가 <code className="doc-code">SOUL.md</code>와 관측
        아카이브를 조회)은 기기를 떠나지 않는다.
      </p>

      <div className="doc-tablewrap">
        <table className="doc-table doc-table-boundary">
          <caption className="doc-caption">§D2 — 전송되는 것 / 전송되지 않는 것</caption>
          <thead>
            <tr>
              <th scope="col">데이터</th>
              <th scope="col">전송 여부</th>
            </tr>
          </thead>
          <tbody>
            {D2_ROWS.map((r, i) => (
              <tr key={i} className={`is-${r.tone}`}>
                <td className={r.strongData ? "is-strong" : undefined}>
                  <Ticks text={r.data} />
                </td>
                <td className={r.strongSent ? "is-strong" : undefined}>
                  <Ticks text={r.sent} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <div className="doc-note">
        <h2 className="doc-h2">검색과 링크는 별도의 노출 경로다 (§D6)</h2>
        <p>
          문화 글귀 생성은 <strong>투입마다 자동으로 일어난다.</strong> 검색어에 무엇에 관심이
          있는지가 직접 드러나며, 그것이 검색 제공자에게 전송된다. 방문한 페이지에도 IP가 남는다.
          즉 <strong>투입 하나가 곧 검색 몇 건</strong>이다. 이 앱을 쓰는 동안 관심사가 검색
          제공자에게 지속적으로 흘러간다. 설계상 불가피한 대가이며 숨기지 않는다.
        </p>
        <p>
          검색어는 <code className="doc-code">context.queries</code>에 전문 기록되므로 무엇이
          나갔는지 언제든 확인할 수 있다.
        </p>
        <p>
          YouTube 해석(oEmbed · Data API · yt-dlp)은 YouTube 서버에 IP를 남긴다. 썸네일은 OpenAI
          서버가 대신 가져오므로 예외다.
        </p>
      </div>

      <div className="doc-note">
        <h2 className="doc-h2">항목별로 "이건 안 보냄"을 두지 않는다 (§D5)</h2>
        <p>
          임베딩이 API를 쓰므로 직접 쓴 문장이라도 결국 전송된다. 지킬 수 없는 약속을 UI로 제시하지
          않는다. <strong>민감한 자료는 애초에 투입하지 않는 것이 유일하게 정직한 방법이다.</strong>
        </p>
      </div>

      <fieldset className="doc-choice">
        <legend className="doc-h2">문화 층을 켜고 시작할까요</legend>
        <p className="doc-muted">
          문화 글귀를 만들 때 <strong>투입할 때마다 웹 검색이 일어납니다.</strong>
        </p>
        <label className="doc-radio">
          <input
            type="radio"
            name="context-enabled"
            checked={contextEnabled}
            onChange={() => setContextEnabled(true)}
          />
          <span>
            <span className="doc-radio-title">켠 채로 시작 (기본값)</span>
            <span className="doc-muted">
              투입마다 검색이 일어납니다. 두 글귀가 짝을 이루므로 2×2 셀이 전부 성립합니다.
            </span>
          </span>
        </label>
        <label className="doc-radio">
          <input
            type="radio"
            name="context-enabled"
            checked={!contextEnabled}
            onChange={() => setContextEnabled(false)}
          />
          <span>
            <span className="doc-radio-title">끄고 시작</span>
            <span className="doc-muted">
              검색 노출이 완전히 사라집니다. 대신 문화 층과 2×2 셀 전체가 비활성화됩니다. 나중에
              설정에서 켤 수 있습니다.
            </span>
          </span>
        </label>
      </fieldset>

      {error !== null && (
        <p className="doc-error" role="alert">
          {error}
        </p>
      )}

      <div className="doc-actions">
        <button type="button" className="doc-btn doc-btn-primary" onClick={confirm} disabled={busy}>
          {busy ? "저장 중…" : "읽었습니다. 시작합니다"}
        </button>
      </div>
    </section>
  );
}

export default BoundaryNotice;
