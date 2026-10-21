import { useEffect, useState } from "react";
import { readAppliedUiScale } from "@/shared/ui-scale";

export function useAppliedUiScale(): number {
  const [scale, setScale] = useState(() =>
    typeof document === "undefined" ? 1 : readAppliedUiScale(document.documentElement),
  );

  useEffect(() => {
    if (typeof document === "undefined") return;
    const root = document.documentElement;
    const update = () => setScale(readAppliedUiScale(root));
    update();

    const observer = new MutationObserver(update);
    observer.observe(root, { attributes: true, attributeFilter: ["data-ui-scale", "style"] });
    return () => observer.disconnect();
  }, []);

  return scale;
}
