/**
 * 줄 단위 diff (§13 화면 4).
 *
 * 라이브러리를 쓰지 않는다. 고전적인 LCS 동적계획법이고, 대상은 `SOUL.md`의
 * 블록 하나 정도라 크기가 작다.
 *
 * 순수 함수다. 여기서 무엇을 승인할지 판단하지 않는다 — 화면은 결과를 보여줄 뿐이고
 * 기록은 `approveProposal`이 한다 (§2).
 */

export type DiffKind = "equal" | "del" | "add";

export interface DiffLine {
  kind: DiffKind;
  /** 왼쪽(현재) 줄 번호. 1-based. 추가된 줄이면 `null` */
  left: number | null;
  /** 오른쪽(제안) 줄 번호. 1-based. 삭제된 줄이면 `null` */
  right: number | null;
  text: string;
}

export interface DiffRow {
  left: DiffLine | null;
  right: DiffLine | null;
}

/** 표 크기 상한. 넘으면 LCS를 포기하고 통짜 교체로 본다 */
const MAX_CELLS = 4_000_000;

export function splitLines(s: string): string[] {
  return s.replace(/\r\n/g, "\n").split("\n");
}

export function diffLines(a: string[], b: string[]): DiffLine[] {
  const out: DiffLine[] = [];

  // 공통 머리 · 꼬리를 먼저 떼면 DP 표가 작아지고 결과도 읽기 좋아진다
  let head = 0;
  while (head < a.length && head < b.length && a[head] === b[head]) head++;
  let tail = 0;
  while (
    tail < a.length - head &&
    tail < b.length - head &&
    a[a.length - 1 - tail] === b[b.length - 1 - tail]
  ) {
    tail++;
  }

  for (let i = 0; i < head; i++) {
    out.push({ kind: "equal", left: i + 1, right: i + 1, text: a[i] });
  }

  const midA = a.slice(head, a.length - tail);
  const midB = b.slice(head, b.length - tail);
  const n = midA.length;
  const m = midB.length;

  if ((n + 1) * (m + 1) > MAX_CELLS) {
    for (let i = 0; i < n; i++) {
      out.push({ kind: "del", left: head + i + 1, right: null, text: midA[i] });
    }
    for (let j = 0; j < m; j++) {
      out.push({ kind: "add", left: null, right: head + j + 1, text: midB[j] });
    }
  } else {
    const w = m + 1;
    const dp = new Int32Array((n + 1) * w);
    for (let i = n - 1; i >= 0; i--) {
      for (let j = m - 1; j >= 0; j--) {
        dp[i * w + j] =
          midA[i] === midB[j]
            ? dp[(i + 1) * w + j + 1] + 1
            : Math.max(dp[(i + 1) * w + j], dp[i * w + j + 1]);
      }
    }
    let i = 0;
    let j = 0;
    while (i < n && j < m) {
      if (midA[i] === midB[j]) {
        out.push({ kind: "equal", left: head + i + 1, right: head + j + 1, text: midA[i] });
        i++;
        j++;
      } else if (dp[(i + 1) * w + j] >= dp[i * w + j + 1]) {
        out.push({ kind: "del", left: head + i + 1, right: null, text: midA[i] });
        i++;
      } else {
        out.push({ kind: "add", left: null, right: head + j + 1, text: midB[j] });
        j++;
      }
    }
    while (i < n) {
      out.push({ kind: "del", left: head + i + 1, right: null, text: midA[i] });
      i++;
    }
    while (j < m) {
      out.push({ kind: "add", left: null, right: head + j + 1, text: midB[j] });
      j++;
    }
  }

  for (let k = 0; k < tail; k++) {
    const li = a.length - tail + k;
    const ri = b.length - tail + k;
    out.push({ kind: "equal", left: li + 1, right: ri + 1, text: a[li] });
  }

  return out;
}

/** 좌우로 나란히 놓기 위해 삭제·추가 묶음을 짝지어 준다. */
export function sideBySide(ops: DiffLine[]): DiffRow[] {
  const rows: DiffRow[] = [];
  let dels: DiffLine[] = [];
  let adds: DiffLine[] = [];

  const flush = () => {
    const n = Math.max(dels.length, adds.length);
    for (let i = 0; i < n; i++) {
      rows.push({ left: dels[i] ?? null, right: adds[i] ?? null });
    }
    dels = [];
    adds = [];
  };

  for (const op of ops) {
    if (op.kind === "del") dels.push(op);
    else if (op.kind === "add") adds.push(op);
    else {
      flush();
      rows.push({ left: op, right: op });
    }
  }
  flush();
  return rows;
}

export interface DiffStat {
  added: number;
  removed: number;
}

export function diffStat(ops: DiffLine[]): DiffStat {
  let added = 0;
  let removed = 0;
  for (const op of ops) {
    if (op.kind === "add") added++;
    else if (op.kind === "del") removed++;
  }
  return { added, removed };
}
