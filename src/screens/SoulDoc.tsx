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
import "../styles/doc.css";

function MarkdownView({ src }: { src: string }) {
  const nodes = useMemo<MdNode[]>(() => parseMarkdown(src), [src]);

  if (nodes.length === 0) {
    return <p className="doc-empty">아직 아무것도 없습니다.</p>;
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

export function SoulDoc() {
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
          <span>저장하지 않은 편집이 있습니다. 버릴까요?</span>
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
            블록 편집 {result.profile_edits}건을 관측으로 기록했습니다 · 커밋 {result.commits}개.
            <span className="doc-muted"> 저장 한 번에 커밋이 여러 개 생기는 것은 정상입니다.</span>
          </p>
          {result.gen_blocks_modified.length > 0 && (
            <p className="doc-warn">
              <strong>재빌드 시 덮어써집니다</strong> — 고친 <code className="doc-code">soul:gen</code>{" "}
              블록: {result.gen_blocks_modified.join(" · ")}
            </p>
          )}
        </div>
      )}

      {editing ? (
        <>
          <p className="doc-muted doc-legend">
            회색 영역은 <code className="doc-code">soul:gen</code> 블록입니다. 파생값으로 다시
            채워지므로 여기에 쓴 것은 재빌드 때 덮어써집니다 (§8.3 규칙 7).
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
