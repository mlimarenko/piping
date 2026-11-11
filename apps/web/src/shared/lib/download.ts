/**
 * Save a text artifact through the browser's download mechanism.
 *
 * The object URL is revoked on the next task so browsers that still read the
 * blob during the click can finish, without keeping the payload alive.
 */
export function downloadTextFile(fileName: string, text: string, mimeType = "text/plain"): void {
  const blob = new Blob([text], { type: `${mimeType};charset=utf-8` });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = fileName;
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 0);
}
