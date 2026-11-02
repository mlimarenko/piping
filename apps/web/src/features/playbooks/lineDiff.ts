/**
 * Minimal LCS-based line diff for comparing two playbook-version scripts.
 *
 * Deliberately dependency-free and small: version contents are capped at 1 MiB
 * by the backend and version counts are low, so an O(n·m) LCS table is fine and
 * keeps the diff view out of any heavy diff library / extra bundle weight.
 */

export type DiffOp = "equal" | "added" | "removed";

export interface DiffLine {
  op: DiffOp;
  /** 1-based line number in the "before" text, when present. */
  oldLine: number | null;
  /** 1-based line number in the "after" text, when present. */
  newLine: number | null;
  text: string;
}

/** Compute a unified line diff between `before` and `after`. */
export function diffLines(before: string, after: string): DiffLine[] {
  const a = before.length === 0 ? [] : before.split("\n");
  const b = after.length === 0 ? [] : after.split("\n");
  const n = a.length;
  const m = b.length;

  // LCS length table.
  const lcs: number[][] = Array.from({ length: n + 1 }, () =>
    new Array<number>(m + 1).fill(0),
  );
  for (let i = n - 1; i >= 0; i -= 1) {
    const lcsI = lcs[i] ?? [];
    const lcsI1 = lcs[i + 1] ?? [];
    for (let j = m - 1; j >= 0; j -= 1) {
      const ai = a[i] ?? "";
      const bj = b[j] ?? "";
      lcsI[j] =
        ai === bj
          ? (lcsI1[j + 1] ?? 0) + 1
          : Math.max(lcsI1[j] ?? 0, lcsI[j + 1] ?? 0);
    }
  }

  const out: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    const ai = a[i] ?? "";
    const bj = b[j] ?? "";
    const lcsI = lcs[i] ?? [];
    const lcsI1 = lcs[i + 1] ?? [];
    if (ai === bj) {
      out.push({ op: "equal", oldLine: i + 1, newLine: j + 1, text: ai });
      i += 1;
      j += 1;
    } else if ((lcsI1[j] ?? 0) >= (lcsI[j + 1] ?? 0)) {
      out.push({ op: "removed", oldLine: i + 1, newLine: null, text: ai });
      i += 1;
    } else {
      out.push({ op: "added", oldLine: null, newLine: j + 1, text: bj });
      j += 1;
    }
  }
  while (i < n) {
    out.push({ op: "removed", oldLine: i + 1, newLine: null, text: a[i] ?? "" });
    i += 1;
  }
  while (j < m) {
    out.push({ op: "added", oldLine: null, newLine: j + 1, text: b[j] ?? "" });
    j += 1;
  }
  return out;
}
