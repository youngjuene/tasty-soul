/**
 * 접어 둔 설명. 화면마다 "이건 무슨 뜻인가요" 하나씩.
 *
 * 이 앱은 처음 열면 할 일이 하나뿐이다 — 무언가 넣고, 두 버튼 중 하나를 누른다.
 * 거기까지는 설명이 필요 없어야 한다. 그런데 대시보드와 아카이브에는 `새로움`,
 * `또렷함`, `무리` 처럼 **처음 보면 모르는 말**이 숫자로 나온다. 그 말을 지우면
 * 앱이 얕아지고, 그 말을 본문에 풀어 쓰면 화면이 문서가 된다.
 *
 * 그래서 접어 둔다. **처음 온 사람은 접힌 줄 하나를 지나칠 뿐이고, 궁금해진
 * 사람만 펼친다.** 궁금해지는 것은 대개 두세 번째 방문이며, 그때가 이 앱이
 * 깊어지기 시작하는 지점이다.
 *
 * 툴팁으로는 이 일을 할 수 없다 — 손가락으로 쓰는 화면에서는 아예 뜨지 않고,
 * 뜬다 해도 "여기에 더 있다"는 사실 자체가 보이지 않는다.
 *
 * `id` 는 같은 값이 `SOUL.md` · MCP · CLI 에서 불리는 이름이다. 펼친 사람에게만
 * 보이면 되지만, **보이긴 해야 한다.** 그 이름이 다음 단계로 가는 손잡이다.
 */
import { josa } from "../lib/types";
import type { TermDef } from "../lib/types";
import "../styles/explain.css";

export interface ExplainProps {
  items: readonly TermDef[];
  /** 접힌 줄에 적힐 말. 화면에 따라 묻는 방식이 다르다. */
  label?: string;
}

/**
 * 명세 참조 하나. `<Ref>§9.7</Ref>` → ` (§9.7)` 을 작고 흐리게.
 *
 * 지우지 않는 이유는 이 앱의 모든 동작에 근거가 있고 그것을 확인할 수 있어야
 * 하기 때문이다. 문장 밖으로 미는 이유는 처음 온 사람에게 `§R11` 이 뜻 없는
 * 잡음이고, 잡음이 문장 한가운데 있으면 문장이 거기서 끊기기 때문이다.
 */
export function Ref({ children }: { children: string }) {
  return <span className="ref">({children})</span>;
}

export default function Explain({ items, label = "이 말들이 무슨 뜻인가요" }: ExplainProps) {
  if (items.length === 0) return null;
  return (
    <details className="explain">
      <summary className="explain-summary">{label}</summary>
      <dl className="explain-list">
        {items.map((t) => (
          <div className="explain-row" key={t.id}>
            <dt className="explain-term">
              {t.name}
              <code className="explain-id">{t.id}</code>
            </dt>
            <dd className="explain-gloss">
              {t.gloss}
              {t.doc && (
                <span className="explain-doc">
                  {`SOUL.md 에서는 «${t.doc}»${josa(t.doc, "이라고", "라고")} 적힙니다.`}
                </span>
              )}
            </dd>
          </div>
        ))}
      </dl>
    </details>
  );
}
