import { API_BASE_URL } from "@/shared/api/client";

/**
 * Build the live MJPEG stream URL for a target/channel. Shared by the single
 * view, grid cells and tests so they agree on the exact shape. `attempt`
 * cache-busts reconnects (the browser would otherwise reuse the failed
 * connection).
 */
export function deviceStreamUrl(
  targetId: string,
  options: Readonly<{ channel?: number; fps?: number; maxWidth?: number; attempt?: number }> = {},
): string {
  const params = new URLSearchParams();
  if (options.channel != null) params.set("channel", String(options.channel));
  if (options.fps != null) params.set("fps", String(options.fps));
  if (options.maxWidth != null) params.set("max_width", String(options.maxWidth));
  if (options.attempt) params.set("t", String(options.attempt));
  const query = params.toString();
  const querySuffix = query ? `?${query}` : "";
  return `${API_BASE_URL}/api/v1/cctv/${encodeURIComponent(targetId)}/stream${querySuffix}`;
}
