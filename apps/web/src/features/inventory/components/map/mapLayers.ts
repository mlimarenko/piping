/**
 * Single source of truth for the map's entity layers. The legend, the composite
 * site-marker ring/badge, and the site breakdown panel all derive their colours,
 * icons, ordering, and inventory deep-links from these registries — adding a new
 * layer later means extending these maps, not touching the render code.
 *
 * Colours are concrete CSS colours because MapLibre paint properties are
 * evaluated inside canvas and cannot resolve CSS variables.
 */
import { Building2, Server } from "lucide-react";
import type { ElementType } from "react";

/** The active map layer ids. `sites` carry coordinates; `devices` are plotted at
 *  their site's coordinates via the composite ring/badge/breakdown. */
export type MapLayerId = "sites" | "devices";

/** Layer ids in visual-weight order (used by the breakdown panel rows and to
 *  keep ring arcs deterministic across renders). */
export const LAYER_DISPLAY_ORDER: readonly MapLayerId[] = ["sites", "devices"];

/** Swatch / ring colour per layer. */
export const LAYER_COLOR: Record<MapLayerId, string> = {
  sites: "#f59e0b",
  devices: "#38bdf8",
};

/** Lucide icon per layer, used in the legend and the breakdown panel rows. */
export const LAYER_ICON: Record<MapLayerId, ElementType> = {
  sites: Building2,
  devices: Server,
};
