import type { CSSProperties } from "react";

const INDENT = 1.5;
const BASE = 0.5;
const SLOT_CENTER = 0.75;

export const TREE_ROW_CONTENT_STYLE = { columnGap: "0.5rem" } satisfies CSSProperties;

export function treeRowStyle(depth: number): CSSProperties {
  return { ...TREE_ROW_CONTENT_STYLE, paddingLeft: `${BASE + depth * INDENT}rem` };
}

export function treeGuideLeft(depth: number): string {
  return `${BASE + SLOT_CENTER + depth * INDENT}rem`;
}

export function treeGuideWidth(): string {
  return `${INDENT}rem`;
}

// Calculate from the complete visible preorder, before slicing virtual rows.
// A flag at level N continues that rail below this row only while the next row
// remains inside the level-N node's subtree. The final flag starts this row's
// own rail when it has visible descendants.
export function treeRowPaths(depths: number[]): boolean[][] {
  return depths.map((depth, index) => {
    const nextDepth = depths[index + 1] ?? -1;
    return Array.from({ length: depth + 1 }, (_, level) => nextDepth > level);
  });
}
