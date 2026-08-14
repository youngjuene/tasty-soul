/**
 * 화면 하나가 던져도 앱 전체가 백지가 되지 않게 한다.
 *
 * ## 왜 필요한가 — 실제로 두 번 겪었다
 *
 * React 는 렌더 중 throw 를 **트리 전체 언마운트**로 처리한다. 경계가 없으면
 * 화면 하나의 사소한 실수(예상치 못한 `null`, 스키마 변경, `undefined.length`)가
 * **아무 메시지 없는 검은 화면**이 된다. 콘솔을 열지 않는 사람에게는 앱이 그냥 죽은 것이다.
 *
 * 이 앱은 특히 위험하다. 화면이 그리는 값은 전부 Rust 파생 층에서 오고(§12),
 * 그 스키마가 한 번이라도 어긋나면 여기서 터진다. 실제로 `Derived.prompt_boundaries`
 * 의 모양이 바뀌었을 때 대시보드가 앱 전체를 백지로 만들었다.
 *
 * **경계는 화면 단위로 세운다.** 셸(탭 바)은 살아 있어야 다른 화면으로 빠져나갈 수 있다.
 * 앱 최상단에만 두면 탭까지 사라져 사용자가 아무것도 못 한다.
 */

import { Component } from "react";
import type { ErrorInfo, ReactNode } from "react";

interface Props {
  /** 어느 화면에서 터졌는지. 사용자에게 그대로 보여준다. */
  screen: string;
  children: ReactNode;
}

interface State {
  error: Error | null;
}

export default class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // stderr 대신 콘솔로. 개발 중에 원인을 찾을 단서를 남긴다.
    console.error(`[${this.props.screen}] 렌더 실패`, error, info.componentStack);
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;

    return (
      <div className="err-boundary" role="alert">
        <h2 className="err-boundary__title">{this.props.screen} 화면을 그리지 못했습니다</h2>
        <p className="err-boundary__body">
          다른 탭은 그대로 쓸 수 있습니다. 이 화면만 실패했습니다.
        </p>
        <pre className="err-boundary__detail">{error.message}</pre>
        <button
          type="button"
          className="ts-btn"
          onClick={() => this.setState({ error: null })}
        >
          다시 시도
        </button>
      </div>
    );
  }
}
