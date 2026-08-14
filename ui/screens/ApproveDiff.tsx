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
import { axisLabel, dashSigned, type ProposalView } from "../lib/types";
import { Ref } from "../components/Explain";
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
      setDone("거절했습니다. 이 제안은 어디에도 남지 않습니다.");
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
      {/*
        무엇이 전송되는지는 줄이지 않는다 (`ui/README.md` 6). 다만 "성찰"이 무엇인지
        모르는 채로 그 경고만 읽으면 무엇을 승인하는지 모르고 누르게 된다.
      */}
      <p className="doc-lead">
        그동안 쌓인 답을 앱이 훑어보고 <code className="doc-code">SOUL.md</code> 를 이렇게 고치자고
        제안하는 자리입니다. <strong>승인해야만 반영됩니다.</strong> 거절하면 아무것도 남지
        않습니다.
      </p>
      <p className="doc-muted doc-legend">
        <strong>지금 성찰 실행</strong>을 누르면 바깥 모델을 부릅니다 —{" "}
        <code className="doc-code">soul:gen</code> · <code className="doc-code">soul:neg</code>{" "}
        블록이 전송됩니다<Ref>§D2</Ref>. 직접 쓴{" "}
        <code className="doc-code">soul:human</code> 블록은 나가지 않습니다<Ref>§D4</Ref>.
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
        done === null && <p className="doc-empty">지금은 제안이 없습니다. 답이 더 쌓이면 앱이 알아서 만듭니다.</p>
      ) : (
        <>
          <div className="doc-panel">
            <h2 className="doc-h2">왜 이렇게 고치자는가</h2>
            <p className="doc-md-p">{proposal.rationale}</p>
          </div>

          <div className="doc-panel-row">
            <div className="doc-panel">
              <h2 className="doc-h2">움직인 축</h2>
              {Object.keys(proposal.axis_delta).length === 0 ? (
                <p className="doc-muted">움직인 축 없음</p>
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
                          {axisLabel(axis)}
                          <code className="ident">{axis}</code>
                        </td>
                        <td className="doc-num">{dashSigned(v)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </div>

            <div className="doc-panel">
              <h2 className="doc-h2">근거로 든 항목 {proposal.cites.length}건</h2>
              {proposal.cites.length === 0 ? (
                <p className="doc-muted">—</p>
              ) : (
                <>
                  {/*
                    맨 ULID 만 늘어놓으면 무엇을 보고 있는지 알 수 없다.
                    "아카이브 검색창에 붙여 넣으라"고 쓰고 싶었지만 **그건 거짓말이다** —
                    `commands.rs: search_corpus` 는 서술문·태그·비평문·정정문만 훑고
                    id 는 열쇠로만 쓴다. 없는 길을 안내하느니 무엇인지만 말한다.
                  */}
                  <p className="doc-muted doc-legend">
                    이 제안이 근거로 삼은 항목마다 붙은 고유 번호입니다. 같은 번호가{" "}
                    <code className="doc-code">SOUL.md</code> 와 MCP 에도 그대로 나옵니다.
                  </p>
                  <ul className="doc-cites">
                    {proposal.cites.map((c) => (
                      <li key={c}>
                        <code className="doc-code">{c}</code>
                      </li>
                    ))}
                  </ul>
                </>
              )}
            </div>
          </div>

          <div className="doc-panel">
            <h2 className="doc-h2">
              변경 <span className="doc-muted">+{stat.added} / −{stat.removed}</span>
            </h2>
            <div className="doc-diffwrap">
              <div className="doc-diff-head">
                <div>지금</div>
                <div>이렇게 하자</div>
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
                고쳐서 승인하기
                <code className="ident">soul:neg id=profile</code>
              </h2>
              <p className="doc-lead">
                여기 적은 대로 기록됩니다. <strong>앱이 제안한 원래 문장은 남지 않습니다.</strong>
              </p>
              <ul className="doc-notes">
                <li>
                  고칠 수 있는 것은 <code className="doc-code">profile</code> 한 덩어리뿐입니다.
                  문서의 다른 부분은 여기서 건드릴 수 없습니다.
                </li>
                <li>한국어 3~6문장으로 씁니다.<Ref>§11.2</Ref></li>
                <li>
                  회색으로 표시되는 <code className="doc-code">soul:gen</code> 은 다시 만들 때마다
                  덮어써지고<Ref>§8.1</Ref>, 직접 쓴{" "}
                  <code className="doc-code">soul:human</code> 은 기록에도 남지 않고 밖으로도 나가지
                  않습니다.<Ref>§D4</Ref>
                </li>
              </ul>
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
