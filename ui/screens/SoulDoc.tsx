/**
 * 화면 3 — `SOUL.md` (§13 화면 3).
 *
 * 마크다운 렌더 뷰 + 편집 모드 토글. 편집 모드에서 `soul:gen` 블록은 회색으로 구분한다.
 * 저장은 `saveSoulMd`가 전부 한다 — 파싱·해시 비교·재렌더·커밋은 Rust에 있다 (§8.4).
 * 프런트는 텍스트를 넘기고 결과를 알릴 뿐이다 (§2).
 */
import { useEffect, useMemo, useRef, useState } from "react";
import { errorText, readSoulMd, saveSoulMd } from "../lib/api";
import type { SaveResult } from "../lib/types";
import { parseMarkdown, scanSoulBlocks, toLines, type MdNode } from "../lib/md";
import { Ref } from "../components/Explain";
import "../styles/doc.css";

function MarkdownView({ src }: { src: string }) {
  const nodes = useMemo<MdNode[]>(() => parseMarkdown(src), [src]);

  if (nodes.length === 0) {
    return <p className="doc-empty">아직 아무것도 없습니다. 답이 쌓이면 앱이 여기를 채웁니다.</p>;
  }

  return (
    <div className="doc-md">
      {nodes.map((n, i) => {
        switch (n.kind) {
          case "heading": {
            const level = Math.min(Math.max(n.level, 1), 6);
            const Tag = `h${level}` as "h1" | "h2" | "h3" | "h4" | "h5" | "h6";
            return (
              <Tag key={i} className={`doc-md-h doc-md-h${level}`}>
                {n.text}
              </Tag>
            );
          }
          case "paragraph":
            return (
              <p key={i} className="doc-md-p">
                {n.text}
              </p>
            );
          case "list":
            return n.ordered ? (
              <ol key={i} className="doc-md-list">
                {n.items.map((it, k) => (
                  <li key={k}>{it}</li>
                ))}
              </ol>
            ) : (
              <ul key={i} className="doc-md-list">
                {n.items.map((it, k) => (
                  <li key={k}>{it}</li>
                ))}
              </ul>
            );
          case "table":
            return (
              <div key={i} className="doc-tablewrap">
                <table className="doc-table">
                  <thead>
                    <tr>
                      {n.head.map((h, k) => (
                        <th key={k} scope="col">
                          {h}
                        </th>
                      ))}
                    </tr>
                  </thead>
                  <tbody>
                    {n.rows.map((row, r) => (
                      <tr key={r}>
                        {row.map((c, k) => (
                          <td key={k}>{c}</td>
                        ))}
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            );
        }
      })}
    </div>
  );
}

export interface SoulDocProps {
  /** 저장이 끝난 뒤. 저장은 `soul_delta` 관측을 만들므로 파생값이 실제로 달라진다. */
  onSaved?: () => void;
  /**
   * 값이 바뀌면 파일을 다시 읽는다. **승인이 이걸 올린다** — 제안을 승인하면
   * `SOUL.md` 가 다시 렌더되는데(§8.4), 그때까지 이 화면은 낡은 문서를 들고 있었다.
   * 같은 화면 아래쪽에서 승인해 놓고 위쪽 문서가 그대로면 승인이 안 먹은 것처럼 보인다.
   *
   * 편집 중에는 무시한다 — 다시 읽으면 사용자가 쓰던 초안이 날아간다.
   */
  refreshKey?: number;
}

export function SoulDoc({ onSaved, refreshKey = 0 }: SoulDocProps = {}) {
  const [text, setText] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [editing, setEditing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<SaveResult | null>(null);
  const [confirmDiscard, setConfirmDiscard] = useState(false);

  const backdropRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    let alive = true;
    readSoulMd()
      .then((s) => {
        if (!alive) return;
        setText(s);
        setDraft(s);
      })
      .catch((e) => {
        if (alive) setError(errorText(e));
      });
    return () => {
      alive = false;
    };
  }, []);

  // 승인 등으로 문서가 밖에서 바뀌었을 때 (마운트 때의 최초 읽기와는 별개다).
  // 자기 저장으로도 한 번 더 돌지만 `save()` 가 이미 같은 값을 넣어 둔 뒤라
  // `setText` 가 같은 문자열을 만나 다시 그리지 않는다 — 로컬 파일 읽기 한 번이 전부다.
  const firstRefresh = useRef(true);
  useEffect(() => {
    if (firstRefresh.current) {
      firstRefresh.current = false;
      return;
    }
    if (editing) return;
    let alive = true;
    readSoulMd()
      .then((fresh) => {
        if (!alive) return;
        setText(fresh);
        setDraft(fresh);
      })
      .catch(() => {
        /* 실패하면 화면에 있던 것을 그대로 둔다. 저장 경로에서 다시 드러난다. */
      });
    return () => {
      alive = false;
    };
    // `editing` 은 의도적으로 뺀다 — 편집을 끝내는 것만으로 다시 읽히면 안 된다.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refreshKey]);

  const dirty = text !== null && draft !== text;

  /** 편집 화면에서 회색으로 덮을 줄 (여는·닫는 마커 포함) */
  const genLines = useMemo(() => {
    const set = new Set<number>();
    for (const b of scanSoulBlocks(draft)) {
      if (b.kind !== "gen") continue;
      const end = b.markerEnd === -1 ? b.markerStart : b.markerEnd;
      for (let i = b.markerStart; i <= end; i++) set.add(i);
    }
    return set;
  }, [draft]);

  const draftLines = useMemo(() => toLines(draft), [draft]);

  async function save() {
    setBusy(true);
    setError(null);
    setResult(null);
    try {
      const r = await saveSoulMd(draft);
      // 저장은 재렌더까지 하므로 (§8.4 5–7단계) 파일을 다시 읽어야 화면과 디스크가 맞는다
      const fresh = await readSoulMd();
      setText(fresh);
      setDraft(fresh);
      setResult(r);
      setEditing(false);
      onSaved?.();
    } catch (e) {
      // §8.4 2단계 — 파싱 실패면 파일을 되돌리지 않는다. 편집 상태를 그대로 둔다
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }

  function leaveEdit() {
    if (dirty) {
      setConfirmDiscard(true);
      return;
    }
    setEditing(false);
  }

  function discard() {
    setDraft(text ?? "");
    setConfirmDiscard(false);
    setEditing(false);
    setError(null);
  }

  if (text === null) {
    return (
      <section className="doc doc-soul">
        {error === null ? (
          <p className="doc-empty">읽는 중…</p>
        ) : (
          <p className="doc-error" role="alert">
            {error}
          </p>
        )}
      </section>
    );
  }

  return (
    <section className="doc doc-soul">
      <header className="doc-head">
        <h1 className="doc-title">SOUL.md</h1>
        <div className="doc-actions">
          {editing ? (
            <>
              <button type="button" className="doc-btn" onClick={leaveEdit} disabled={busy}>
                편집 끝내기
              </button>
              <button
                type="button"
                className="doc-btn doc-btn-primary"
                onClick={save}
                disabled={busy || !dirty}
              >
                {busy ? "저장 중…" : "저장"}
              </button>
            </>
          ) : (
            <button
              type="button"
              className="doc-btn"
              onClick={() => {
                setResult(null);
                setError(null);
                setEditing(true);
              }}
            >
              편집
            </button>
          )}
        </div>
      </header>

      {confirmDiscard && (
        <div className="doc-confirm" role="alert">
          <span>저장하지 않은 것이 있습니다. 버릴까요?</span>
          <button type="button" className="doc-btn doc-btn-danger" onClick={discard}>
            버리기
          </button>
          <button type="button" className="doc-btn" onClick={() => setConfirmDiscard(false)}>
            계속 편집
          </button>
        </div>
      )}

      {error !== null && (
        <p className="doc-error" role="alert">
          {error}
        </p>
      )}

      {result !== null && (
        <div className="doc-result" role="status">
          <p>
            고친 곳 {result.profile_edits}군데를 기록했습니다 · 저장 기록 {result.commits}개.
            <span className="doc-muted"> 한 번 저장에 기록이 여러 개 생기는 것은 정상입니다.</span>
          </p>
          {result.gen_blocks_modified.length > 0 && (
            <p className="doc-warn">
              <strong>여기 쓴 것은 다음에 다시 만들 때 지워집니다</strong> — 방금 고친 곳이 앱이
              채우는 자리(<code className="doc-code">soul:gen</code>)였습니다:{" "}
              {result.gen_blocks_modified.join(" · ")}
            </p>
          )}
        </div>
      )}

      {editing ? (
        <>
          <p className="doc-muted doc-legend">
            <strong>회색으로 덮인 줄은 앱이 채우는 자리입니다.</strong> 쌓인 답에서 다시 계산해
            채우므로, 거기에 쓴 것은 다음에 다시 만들 때 지워집니다. 남기고 싶은 문장은 회색이
            아닌 곳에 쓰세요.<Ref>§8.3 규칙 7</Ref>
          </p>
          <div className="doc-edit">
            <div className="doc-edit-backdrop" ref={backdropRef} aria-hidden="true">
              {draftLines.map((l, i) => (
                <div key={i} className={genLines.has(i) ? "doc-edit-line is-gen" : "doc-edit-line"}>
                  {l === "" ? " " : l}
                </div>
              ))}
            </div>
            <textarea
              className="doc-edit-input"
              value={draft}
              spellCheck={false}
              onChange={(e) => setDraft(e.target.value)}
              onScroll={(e) => {
                const el = backdropRef.current;
                if (el === null) return;
                el.scrollTop = e.currentTarget.scrollTop;
                el.scrollLeft = e.currentTarget.scrollLeft;
              }}
            />
          </div>
        </>
      ) : (
        <MarkdownView src={text} />
      )}
    </section>
  );
}

export default SoulDoc;
