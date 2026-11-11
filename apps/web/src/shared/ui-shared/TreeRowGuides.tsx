import { treeGuideLeft, treeGuideWidth } from "./treeRowGeometry";

export function TreeRowGuides({ path, expanded, activeRails = [], activeBranch = false }: { path: boolean[]; expanded: boolean; activeRails?: { level: number; top: boolean; bottom: boolean }[]; activeBranch?: boolean }) {
  const depth = path.length - 1;
  return (
    <span aria-hidden="true" className="pointer-events-none absolute inset-0 overflow-hidden">
      {path.slice(0, depth).map((continues, level) => {
        return (
          <span
            key={level}
            className="absolute top-0 border-l border-border"
            style={{
              left: treeGuideLeft(level),
              bottom: continues ? 0 : "50%",
              transform: "translateX(-0.5px)",
            }}
          />
        );
      })}
      {depth > 0 && (
        <span
          className="absolute top-1/2 border-t border-border"
          style={{
            left: treeGuideLeft(depth - 1),
            width: treeGuideWidth(),
            transform: "translateY(-0.5px)",
          }}
        />
      )}
      {expanded && path[depth] && (
        <span
          className="absolute bottom-0 top-1/2 border-l border-border"
          style={{ left: treeGuideLeft(depth), transform: "translateX(-0.5px)" }}
        />
      )}
      {activeRails.map(({ level, top, bottom }) => (
        <span key={level} className="absolute border-l border-primary/70"
          style={{ left: treeGuideLeft(level), top: top ? 0 : "50%", bottom: bottom ? 0 : "50%", transform: "translateX(-0.5px)" }} />
      ))}
      {activeBranch && depth > 0 && (
        <span className="absolute top-1/2 border-t border-primary/70"
          style={{ left: treeGuideLeft(depth - 1), width: treeGuideWidth(), transform: "translateY(-0.5px)" }} />
      )}
    </span>
  );
}
