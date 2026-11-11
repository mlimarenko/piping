/** Pure geometry for the Guacamole display viewport. */

export interface SessionDisplaySize {
  width: number;
  height: number;
}

/**
 * Shrink a requested remote size to a maximum while preserving its aspect
 * ratio. A size already within the limit is returned unchanged.
 */
export function capDisplayResolution(
  size: SessionDisplaySize,
  max: SessionDisplaySize | null | undefined,
): SessionDisplaySize {
  if (!max || size.width <= 0 || size.height <= 0) return size;
  const scale = Math.min(1, max.width / size.width, max.height / size.height);
  if (scale >= 1) return size;
  return {
    width: Math.max(1, Math.round(size.width * scale)),
    height: Math.max(1, Math.round(size.height * scale)),
  };
}

/**
 * Uniform scale that fits the remote framebuffer into the panel without
 * distorting it. Zero or negative sizes yield scale 1.
 */
export function fitDisplayScale(
  container: SessionDisplaySize,
  display: SessionDisplaySize,
): number {
  if (container.width <= 0 || container.height <= 0 || display.width <= 0 || display.height <= 0) {
    return 1;
  }
  return Math.min(container.width / display.width, container.height / display.height);
}
