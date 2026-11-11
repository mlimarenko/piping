import { useQuery } from "@tanstack/react-query";

import { previewPlaybookTargetSet } from "./api";
import type { TargetSetPreviewState } from "./TargetSetPicker";

/** Resolved composition of one target set for the picker preview. */
export function useTargetSetPreview(targetSetId: string, canRead: boolean): TargetSetPreviewState {
  const preview = useQuery({
    queryKey: ["playbooks", "target-set-preview", targetSetId],
    queryFn: () => previewPlaybookTargetSet(targetSetId, 50),
    enabled: canRead && Boolean(targetSetId),
  });
  if (!targetSetId) return { kind: "idle" };
  if (!canRead) return { kind: "denied" };
  if (preview.isError) return { kind: "error" };
  if (!preview.data) return { kind: "pending" };
  return { kind: "ready", total: preview.data.total };
}
