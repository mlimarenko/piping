import { describe, expect, it } from "vitest";
import {
  CUSTOM_ICON_FIELD,
  extractPersistedIcon,
  isSafeIconDataUrl,
  mergePersistedIcon,
} from "@/shared/entity-icon-utils";

const PNG_DATA_URL = "data:image/png;base64,aGVsbG8=";

describe("entity icon persistence helpers", () => {
  it("extracts a persisted pack icon and custom image", () => {
    expect(
      extractPersistedIcon({
        [CUSTOM_ICON_FIELD]: { name: "server", data_url: PNG_DATA_URL },
      }),
    ).toEqual({ name: "server", dataUrl: PNG_DATA_URL });
  });

  it("rejects unsafe custom image data urls", () => {
    expect(isSafeIconDataUrl("data:image/svg+xml;base64,PHN2Zy8+")).toBe(false);
    expect(
      extractPersistedIcon({
        [CUSTOM_ICON_FIELD]: {
          name: "server",
          data_url: "data:image/svg+xml;base64,PHN2Zy8+",
        },
      }),
    ).toEqual({ name: "server", dataUrl: undefined });
  });

  it("merges and clears the icon without dropping other custom fields", () => {
    const merged = mergePersistedIcon(
      { owner: "network", [CUSTOM_ICON_FIELD]: { name: "router" } },
      { name: "database", dataUrl: PNG_DATA_URL },
    );
    expect(merged).toEqual({
      owner: "network",
      [CUSTOM_ICON_FIELD]: { name: "database", data_url: PNG_DATA_URL },
    });

    expect(mergePersistedIcon(merged, {})).toEqual({ owner: "network" });
  });
});
