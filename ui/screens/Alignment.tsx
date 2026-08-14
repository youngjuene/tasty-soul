/**
 * 화면 3 곁 — 어긋남 (§12.6 보조 실수 지표).
 *
 * `SOUL.md` 의 `## 어긋남` 블록이 한 줄로 적는 것(`일관성 x · 정정 N건`)을 화면에서
 * 편다. **문서가 방금 바뀌었으면 이 숫자도 방금 바뀐 것이다** — 그래서 같은 화면에
 * 둔다. 문서를 저장하거나 제안을 승인하면 `refreshKey` 가 올라가고 다시 읽는다.
 *
 * ## 왜 이게 나쁜 값이 아닌가
 *
 * 이 앱은 어긋남을 줄이려고 만든 물건이 아니다. §12.6 이 2×2 중 `other_reason`
 * — "보는 것은 같으나 끌리는 이유가 다르다" — 을 **이 시스템이 찾으려는 것**이라고
 * 못박는다. 어긋남이 0이면 기계가 이미 다 맞혔다는 뜻이고, 그러면 배울 것이 없다.
 *
 * 그래서 이 패널의 주인공은 어긋남의 **크기**가 아니라 **방향**이다. 고쳐 쓴
 * 문장들이 한쪽으로 몰려 있으면(`coherence ≥ 0.4`, `|R| ≥ 5` → `systematic`)
 * 그것은 잡음이 아니라 **일관된 차이**이고, 성찰 에이전트가 `SOUL.md` 를 고치자고
 * 제안할 때 근거로 쓰는 것이 바로 그 값이다. 화면은 그 사실을 말해 줘야 한다.
 *
 * ## `ui/README.md` 5 를 어기는 것 아닌가
 *
 * 아니다. 그 규칙은 **대시보드**(화면 5)에 실수 지표를 얹지 말라는 것이고, 이유는
 * 연속값 둘이 2×2 격자와 자리를 다투면 격자가 늦게 읽히기 때문이다. 여기에는 격자가
 * 없고, 바로 위에 같은 숫자를 한 줄로 적은 문서가 있다. 화면 5는 그대로 격자만 둔다.
 *
 * 계산은 전부 Rust 가 했다. 이 파일은 뷰다 (§2).
 */
import { useEffect, useState } from "react";
import { dashboard, errorText } from "../lib/api";
import { dash, EM_DASH, TERMS } from "../lib/types";
import type { Coherence, Derived, Layer } from "../lib/types";
import Explain from "../components/Explain";
import "../styles/doc.css";

export interface AlignmentProps {
  /**
   * 값이 바뀌면 다시 읽는다. 문서를 저장했거나 제안을 승인했을 때 셸이 올린다 —
   * 둘 다 관측을 만들므로 여기 숫자가 실제로 달라진다.
   */
  refreshKey?: number;
}

const LAYER_LABEL: Record<Layer, string> = {
  sensory: "감각 글귀",
  cultural: "문화 글귀",
};

/** §12.6 — `systematic` 판정에 쓰이는 문턱. 화면은 이 값을 **다시 재지 않고** 적기만 한다. */
const SYSTEMATIC_MIN_VALUE = "0.40";
const SYSTEMATIC_MIN_SAMPLE = 5;

/**
 * 한 층의 방향 한 줄.
 *
 * 세 갈래다. `null`(잴 수 없음) / 재긴 했으나 아직 방향이라 부르기 이름(`systematic:false`) /
 * 방향이 있음. **셋을 뭉뚱그리면 안 된다** — "0.21" 만 적으면 낮아서 나쁜 것처럼
 * 보이지만 실제로는 "아직 모르겠다"는 뜻이다.
 */
function Direction({ layer, c }: { layer: Layer; c: Coherence | null }) {
  const label = LAYER_LABEL[layer];

  if (c === null) {
    return (
      <div className="align-row">
        <span className="align-layer">{label}</span>
        <div className="align-body">
          <p className="align-verdict is-none">아직 잴 수 없습니다</p>
          <p className="doc-muted align-note">
            고쳐 쓴 문장이 2건은 있어야 방향을 잽니다. «아니다»에 답할 때 한 줄 적어
            두면 그때부터 쌓입니다.
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className="align-row">
      <span className="align-layer">{label}</span>
      <div className="align-body">
        <p className={c.systematic ? "align-verdict is-yes" : "align-verdict is-soon"}>
          {c.systematic ? "방향이 있습니다" : "아직 방향이라 부를 만큼은 아닙니다"}
        </p>
        <p className="align-nums">
          {`${TERMS.coherence.name} ${dash(c.value)} · 고쳐 쓴 것 ${c.sample}건`}
        </p>
        <p className="doc-muted align-note">
          {c.systematic
            ? "빗나가는 방향이 일정합니다. 우연이 아니라 일관된 차이라는 뜻이고, 성찰이 SOUL.md 를 고치자고 제안할 때 근거로 삼는 것이 이것입니다."
            : `고쳐 쓴 것이 ${SYSTEMATIC_MIN_SAMPLE}건 이상이고 ${TERMS.coherence.name}이 ${SYSTEMATIC_MIN_VALUE} 을 넘으면 방향이 있다고 봅니다.`}
        </p>
      </div>
    </div>
  );
}

type Load =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ok"; derived: Derived };

export function Alignment({ refreshKey = 0 }: AlignmentProps = {}) {
  const [load, setLoad] = useState<Load>({ status: "loading" });

  useEffect(() => {
    let alive = true;
    setLoad({ status: "loading" });
    dashboard()
      .then((derived) => alive && setLoad({ status: "ok", derived }))
      .catch((e: unknown) => alive && setLoad({ status: "error", message: errorText(e) }));
    return () => {
      alive = false;
    };
  }, [refreshKey]);

  if (load.status === "loading") {
    return (
      <section className="doc doc-align">
        <p className="doc-empty">읽는 중…</p>
      </section>
    );
  }

  if (load.status === "error") {
    return (
      <section className="doc doc-align">
        <h1 className="doc-title">어긋남</h1>
        <p className="doc-error" role="alert">
          {load.message}
        </p>
      </section>
    );
  }

  const d = load.derived;

  return (
    <section className="doc doc-align" aria-labelledby="align-title">
      <header className="doc-head">
        <h1 id="align-title" className="doc-title">
          어긋남
        </h1>
      </header>

      {/*
        첫 문장이 이 패널 전체의 뜻이다. 숫자보다 먼저 읽혀야 한다 — 안 그러면
        0.31 을 보고 "31% 틀렸네" 로 읽고, 그 순간 이 앱이 무엇을 하려는 물건인지
        정반대로 이해하게 된다.
      */}
      <p className="doc-lead">
        기계가 읽은 것과 내가 뜻한 것 사이의 거리입니다.{" "}
        <strong>높다고 나쁜 것이 아닙니다.</strong> 오히려 여기가 이 앱이 나를 배우는
        자리입니다 — 어긋남이 0이면 기계가 이미 다 맞혔다는 뜻이고, 그러면 새로 알아낼
        것도 없습니다.
      </p>

      <dl className="align-top">
        <div>
          <dt>{TERMS.misread.name}</dt>
          <dd>{dash(d.misread_ratio)}</dd>
        </div>
        <div>
          <dt>{TERMS.corrections.name}</dt>
          {/* 개수는 측정된 값이라 §R10 의 `—` 대상이 아니다. 0이면 `0건`이다. */}
          <dd>{`${d.corrections_total}건`}</dd>
        </div>
      </dl>

      <div className="doc-panel">
        <h2 className="doc-h2">어긋남이 한쪽으로 쏠려 있나</h2>
        <p className="doc-muted doc-legend">
          같은 크기라도 <b>아무 데로나 빗나간 것</b>과 <b>늘 같은 쪽으로 빗나간 것</b>은
          전혀 다릅니다. 뒤쪽만이 배울 수 있는 것입니다.
        </p>
        <Direction layer="sensory" c={d.coherence_sensory} />
        <Direction layer="cultural" c={d.coherence_cultural} />
      </div>

      <p className="align-recent">
        <span className="doc-muted">최근 30일 {TERMS.divergence.name}</span>
        {` 감각 ${dash(d.divergence_sensory)} · 문화 ${dash(d.divergence_cultural)}`}
        {d.divergence_sensory === null && d.divergence_cultural === null && (
          <span className="doc-muted">{` ${EM_DASH} 30일 안에 고쳐 쓴 것이 없습니다`}</span>
        )}
      </p>

      <Explain items={[TERMS.misread, TERMS.corrections, TERMS.coherence, TERMS.divergence]} />
    </section>
  );
}

export default Alignment;
