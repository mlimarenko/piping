import type { useTranslation } from "react-i18next";

// ---------------------------------------------------------------------------
// Artifact formatting — type label and human-readable size
// ---------------------------------------------------------------------------

export function formatArtifactTypeLabel(
  artifactType: string,
  t: ReturnType<typeof useTranslation>["t"],
): string {
  const key = `playbooks.runDetail.artifactType.${artifactType}`;
  const translated = t(key);
  // Fall back to human-readable form if no translation exists.
  return translated === key ? artifactType.replace(/_/g, " ") : translated;
}

export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
