/**
 * 화면 4 — 승인 diff (§13 화면 4).
 *
 * 성찰 제안이 있으면 좌우 diff를 보여주고 `승인` / `거절` / `수정 후 승인`을 받는다.
 * 수정 후 승인은 수정된 텍스트로 `soul_delta`를 기록한다 (§6.6).
 *
 * **편집 상자는 `*_profile_text` 에만 바인딩한다 (§D4·§18-4·T29).**
 * 좌우 diff 표시는 전문(`*_text`)을 쓴다 — 목적지가 로컬 화면이라 §D4 대상이 아니다.
 * 편집 대상까지 전문으로 주면 사용자가 고친 `soul:human` 이 승인 경로로 되돌아가
 * `soul_delta.blocks.profile.to_text` 에 실리고, `profile` 은 `soul:neg` 이라
 * **다음 성찰 호출부터 원격 모델에게 전송된다.** 나간 뒤에는 되돌릴 수 없다.
 *
 * diff는 `src/lib/diff.ts`의 줄 단위 LCS다. 라이브러리를 쓰지 않는다.
 * 축 변화·근거·이유는 커맨드가 준 값을 그대로 놓는다 — 프런트가 계산하지 않는다 (§2).
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { approveProposal, errorText, reflect, rejectProposal } from "../lib/api";
import { dashSigned, type ProposalView } from "../lib/types";
import { diffLines, diffStat, sideBySide, splitLines } from "../lib/diff";
import "../styles/doc.css";

export function ApproveDiff() {
  const [proposal, setProposal] = useState<ProposalView | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const [modifying, setModifying] = useState(false);
  const [modified, setModified] = useState("");

  const load = useCallback(async (force: boolean) => {
    setLoading(true);
    setError(null);
    setDone(null);
    setModifying(false);
    try {
      const p = await reflect(force);
      setProposal(p ?? null);
      // 편집의 출발점은 제안된 `profile` 본문이다. 전문이 아니다 (§D4).
      setModified(p?.proposed_profile_text ?? "");
    } catch (e) {
      setError(errorText(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load(false);
  }, [load]);

  const ops = useMemo(() => {
    if (proposal === null) return [];
    return diffLines(splitLines(proposal.current_text), splitLines(proposal.proposed_text));
  }, [proposal]);

  const rows = useMemo(() => sideBySide(ops), [ops]);
  const stat = useMemo(() => diffStat(ops), [ops]);

  async function approve(text: string | null) {
    setBusy(true);
    setError(null);
    try {
      const id = await approveProposal(text);
      setProposal(null);
      setModifying(false);
      setDone(`승인했습니다 · ${id}`);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }

  async function reject() {
    setBusy(true);
    setError(null);
    try {
      await rejectProposal();
      setProposal(null);
      setModifying(false);
      setDone("거절했습니다. 이 제안은 기록되지 않습니다.");
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="doc doc-diff">
      <header className="doc-head">
        <h1 className="doc-title">성찰 제안</h1>
        <div className="doc-actions">
          <button type="button" className="doc-btn" onClick={() => void load(false)} disabled={busy}>
            다시 확인
          </button>
          <button type="button" className="doc-btn" onClick={() => void load(true)} disabled={busy}>
            지금 성찰 실행
          </button>
        </div>
      </header>
      <p className="doc-muted doc-legend">
        <strong>지금 성찰 실행</strong>은 모델을 호출합니다 —{" "}
        <code className="doc-code">soul:gen</code> · <code className="doc-code">soul:neg</code>{" "}
        블록이 전송됩니다 (§D2). <code className="doc-code">soul:human</code>은 나가지 않습니다
        (§D4).
      </p>

      {error !== null && (
        <p className="doc-error" role="alert">
          {error}
        </p>
      )}

      {done !== null && (
        <p className="doc-result" role="status">
          {done}
        </p>
      )}

      {loading ? (
        <p className="doc-empty">확인 중…</p>
      ) : proposal === null ? (
        done === null && <p className="doc-empty">지금은 대기 중인 제안이 없습니다.</p>
      ) : (
        <>
          <div className="doc-panel">
            <h2 className="doc-h2">이유</h2>
            <p className="doc-md-p">{proposal.rationale}</p>
          </div>

          <div className="doc-panel-row">
            <div className="doc-panel">
              <h2 className="doc-h2">축 변화</h2>
              {Object.keys(proposal.axis_delta).length === 0 ? (
                <p className="doc-muted">축 변화 없음</p>
              ) : (
                <table className="doc-table doc-table-axis">
                  <thead>
                    <tr>
                      <th scope="col">축</th>
                      <th scope="col">변화</th>
                    </tr>
                  </thead>
                  <tbody>
                    {Object.entries(proposal.axis_delta).map(([axis, v]) => (
                      <tr key={axis}>
                        <td>
                          <code className="doc-code">{axis}</code>
                        </td>
                        <td className="doc-num">{dashSigned(v)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </div>

            <div className="doc-panel">
              <h2 className="doc-h2">근거 {proposal.cites.length}건</h2>
              {proposal.cites.length === 0 ? (
                <p className="doc-muted">—</p>
              ) : (
                <ul className="doc-cites">
                  {proposal.cites.map((c) => (
                    <li key={c}>
                      <code className="doc-code">{c}</code>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          </div>

          <div className="doc-panel">
            <h2 className="doc-h2">
              변경 <span className="doc-muted">+{stat.added} / −{stat.removed}</span>
            </h2>
            <div className="doc-diffwrap">
              <div className="doc-diff-head">
                <div>현재</div>
                <div>제안</div>
              </div>
              <div className="doc-diff-body">
                {rows.map((r, i) => (
                  <div className="doc-diff-row" key={i}>
                    <div
                      className={
                        r.left === null
                          ? "doc-diff-cell is-blank"
                          : r.left.kind === "equal"
                            ? "doc-diff-cell"
                            : "doc-diff-cell is-del"
                      }
                    >
                      <span className="doc-diff-n">{r.left?.left ?? ""}</span>
                      <span className="doc-diff-t">{r.left?.text ?? ""}</span>
                    </div>
                    <div
                      className={
                        r.right === null
                          ? "doc-diff-cell is-blank"
                          : r.right.kind === "equal"
                            ? "doc-diff-cell"
                            : "doc-diff-cell is-add"
                      }
                    >
                      <span className="doc-diff-n">{r.right?.right ?? ""}</span>
                      <span className="doc-diff-t">{r.right?.text ?? ""}</span>
                    </div>
                  </div>
                ))}
              </div>
            </div>
          </div>

          {modifying && (
            <div className="doc-panel">
              <h2 className="doc-h2">
                수정 <code className="doc-code">soul:neg id=profile</code>
              </h2>
              <p className="doc-muted doc-legend">
                여기서 고친 내용이 그대로 기록됩니다. 제안 원문은 남지 않습니다. 편집 대상은{" "}
                <code className="doc-code">profile</code> 블록 본문뿐입니다 — 문서의 다른 부분은
                여기서 고칠 수 없습니다. <code className="doc-code">soul:gen</code>은 재빌드가
                덮어쓰고(§8.1), <code className="doc-code">soul:human</code>은 관측에 기록되지도
                원격으로 나가지도 않습니다 (§D4). 한국어 3~6문장이어야 합니다 (§11.2).
              </p>
              <textarea
                className="doc-textarea"
                value={modified}
                spellCheck={false}
                aria-label="profile 블록 본문"
                onChange={(e) => setModified(e.target.value)}
              />
            </div>
          )}

          <div className="doc-actions doc-actions-main">
            {modifying ? (
              <>
                <button
                  type="button"
                  className="doc-btn"
                  onClick={() => {
                    setModifying(false);
                    setModified(proposal.proposed_profile_text);
                  }}
                  disabled={busy}
                >
                  수정 취소
                </button>
                <button
                  type="button"
                  className="doc-btn doc-btn-primary"
                  onClick={() => void approve(modified)}
                  disabled={busy}
                >
                  이 내용으로 승인
                </button>
              </>
            ) : (
              <>
                <button
                  type="button"
                  className="doc-btn doc-btn-danger"
                  onClick={() => void reject()}
                  disabled={busy}
                >
                  거절
                </button>
                <button
                  type="button"
                  className="doc-btn"
                  onClick={() => setModifying(true)}
                  disabled={busy}
                >
                  수정 후 승인
                </button>
                <button
                  type="button"
                  className="doc-btn doc-btn-primary"
                  onClick={() => void approve(null)}
                  disabled={busy}
                >
                  승인
                </button>
              </>
            )}
          </div>
        </>
      )}
    </section>
  );
}

export default ApproveDiff;
