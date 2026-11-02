import { describe, expect, it } from "vitest";

import { ApiError } from "@/shared/api/client";

import { parseDeleteBlockedDetails } from "./deleteBlockers";

describe("parseDeleteBlockedDetails", () => {
  it("parses a structured delete-blocked payload", () => {
    const error = new ApiError({
      status: 409,
      code: "DELETE_BLOCKED",
      reasonCode: "DELETE_BLOCKED",
      message: "Cannot delete this record while related objects still exist",
      details: {
        total_count: 3,
        blockers: [
          {
            table: "dcim_device",
            entity_key: "Devices",
            count: 2,
            examples: [{ id: "dev-1", label: "router01" }],
          },
          {
            table: "mystery_table",
            count: 1,
            examples: [{ label: "opaque row" }],
          },
        ],
      },
    });

    expect(parseDeleteBlockedDetails(error)).toEqual({
      totalCount: 3,
      blockers: [
        {
          table: "dcim_device",
          entityKey: "Devices",
          count: 2,
          examples: [{ id: "dev-1", label: "router01" }],
        },
        {
          table: "mystery_table",
          entityKey: undefined,
          count: 1,
          examples: [{ label: "opaque row" }],
        },
      ],
    });
  });

  it("accepts code-only and camelCase delete-blocked details", () => {
    const error = new ApiError({
      status: 409,
      code: "DELETE_BLOCKED",
      message: "Cannot delete this record while related objects still exist",
      details: {
        totalCount: 1,
        blockers: [
          {
            table: "dcim_device",
            entityKey: "Devices",
            count: 1,
            examples: [{ id: "dev-1", label: "router01" }],
          },
        ],
      },
    });

    expect(parseDeleteBlockedDetails(error)).toEqual({
      totalCount: 1,
      blockers: [
        {
          table: "dcim_device",
          entityKey: "Devices",
          count: 1,
          examples: [{ id: "dev-1", label: "router01" }],
        },
      ],
    });
  });

  it("accepts reason-only delete-blocked details", () => {
    const error = new ApiError({
      status: 409,
      reasonCode: "DELETE_BLOCKED",
      message: "Cannot delete this record while related objects still exist",
      details: {
        total_count: 1,
        blockers: [
          {
            table: "dcim_rack",
            count: 1,
            examples: [{ label: "rack01" }],
          },
        ],
      },
    });

    expect(parseDeleteBlockedDetails(error)).toEqual({
      totalCount: 1,
      blockers: [
        {
          table: "dcim_rack",
          entityKey: undefined,
          count: 1,
          examples: [{ label: "rack01" }],
        },
      ],
    });
  });

  it("returns null for malformed delete-blocked details", () => {
    const error = new ApiError({
      status: 409,
      code: "DELETE_BLOCKED",
      reasonCode: "DELETE_BLOCKED",
      message: "Cannot delete this record while related objects still exist",
      details: {
        total_count: "3",
        blockers: "bad",
      },
    });

    expect(parseDeleteBlockedDetails(error)).toBeNull();
  });
});
