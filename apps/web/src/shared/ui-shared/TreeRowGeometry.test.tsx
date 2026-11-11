import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { TreeRowGuides } from "./TreeRowGuides";
import { treeRowPaths, treeRowStyle } from "./treeRowGeometry";

describe("tree row geometry", () => {
  it("keeps a 24px step beyond three levels, including leaves", () => {
    expect([0, 1, 2, 3, 4, 5].map((depth) => treeRowStyle(depth).paddingLeft)).toEqual([
      "0.5rem", "2rem", "3.5rem", "5rem", "6.5rem", "8rem",
    ]);
  });

  it("continues every ancestor rail through a last-child group to its final visible descendant", () => {
    const paths = treeRowPaths([0, 1, 2, 3]);
    const group = render(<TreeRowGuides path={paths[2]!} expanded />);
    const groupRails = Array.from(group.container.querySelectorAll<HTMLElement>(".border-l"));
    expect(groupRails).toHaveLength(3);
    expect(groupRails.slice(0, 2).map((rail) => [rail.style.left, rail.style.bottom])).toEqual([
      ["1.25rem", "0px"],
      ["2.75rem", "0px"],
    ]);
    expect(groupRails[2]?.style.left).toBe("4.25rem");
    expect(groupRails[2]).toHaveClass("top-1/2", "bottom-0");
    group.unmount();

    const leaf = render(<TreeRowGuides path={paths[3]!} expanded={false} />);
    const leafRails = Array.from(leaf.container.querySelectorAll<HTMLElement>(".border-l"));
    expect(leafRails.map((rail) => [rail.style.left, rail.style.bottom])).toEqual([
      ["1.25rem", "50%"],
      ["2.75rem", "50%"],
      ["4.25rem", "50%"],
    ]);
  });

  it("ends only the completed subtrees and does not connect separate roots", () => {
    // Two child branches under one root, followed by another root and its child.
    const paths = treeRowPaths([0, 1, 2, 1, 2, 2, 0, 1]);
    const firstLeaf = render(<TreeRowGuides path={paths[2]!} expanded={false} />);
    expect(Array.from(firstLeaf.container.querySelectorAll<HTMLElement>(".border-l"))
      .map((rail) => rail.style.bottom)).toEqual(["0px", "50%"]);
    firstLeaf.unmount();

    const lastBranch = render(<TreeRowGuides path={paths[3]!} expanded />);
    const lines = lastBranch.container.querySelectorAll<HTMLElement>(".border-l");
    expect(lines[0]?.style.bottom).toBe("0px");
    expect(lines[1]?.style.left).toBe("2.75rem");
    expect(lines[0]?.style.transform).toBe("translateX(-0.5px)");
    expect(lines[1]?.style.transform).toBe("translateX(-0.5px)");
    lastBranch.unmount();

    const lastLeaf = render(<TreeRowGuides path={paths[5]!} expanded={false} />);
    expect(Array.from(lastLeaf.container.querySelectorAll<HTMLElement>(".border-l"))
      .map((rail) => rail.style.bottom)).toEqual(["50%", "50%"]);
    const elbow = lastLeaf.container.querySelector<HTMLElement>(".border-t");
    expect(elbow?.style.left).toBe("2.75rem");
    expect(elbow?.style.width).toBe("1.5rem");
    expect(elbow?.style.transform).toBe("translateY(-0.5px)");
    lastLeaf.unmount();

    const nextRoot = render(<TreeRowGuides path={paths[6]!} expanded />);
    const rootRails = nextRoot.container.querySelectorAll<HTMLElement>(".border-l");
    expect(rootRails).toHaveLength(1);
    expect(rootRails[0]).not.toHaveClass("top-0");
    expect(nextRoot.container.querySelector(".border-t")).toBeNull();
  });

  it("keeps ancestor guides in a virtual slice and recomputes them after collapse", () => {
    const visible = [0, 1, 2, 3, 4, 5, 5, 1];
    const paths = treeRowPaths(visible);
    const mounted = paths.slice(4, 7);
    expect(mounted[0]).toEqual([true, true, true, true, true]);
    const deep = render(<TreeRowGuides path={mounted[1]!} expanded={false} />);
    expect(Array.from(deep.container.querySelectorAll<HTMLElement>(".border-l"))
      .map((rail) => rail.style.bottom)).toEqual(["0px", "0px", "0px", "0px", "0px"]);
    expect(deep.container.querySelector<HTMLElement>(".border-t")?.style.left).toBe("7.25rem");
    deep.unmount();

    const subtreeEnd = render(<TreeRowGuides path={mounted[2]!} expanded={false} />);
    expect(Array.from(subtreeEnd.container.querySelectorAll<HTMLElement>(".border-l"))
      .map((rail) => rail.style.bottom)).toEqual(["0px", "50%", "50%", "50%", "50%"]);
    subtreeEnd.unmount();

    // Collapse the depth-two branch: its descendants disappear from the preorder.
    const collapsed = treeRowPaths([0, 1, 2, 1]);
    expect(collapsed).toHaveLength(4);
    expect(collapsed[2]).toEqual([true, false, false]);
    const branch = render(<TreeRowGuides path={collapsed[2]!} expanded={false} />);
    expect(Array.from(branch.container.querySelectorAll<HTMLElement>(".border-l"))
      .map((rail) => rail.style.bottom)).toEqual(["0px", "50%"]);
    branch.unmount();

    const collapsedRoot = render(<TreeRowGuides path={treeRowPaths([0, 0])[0]!} expanded={false} />);
    expect(collapsedRoot.container.querySelectorAll(".border-l")).toHaveLength(0);
  });
});
