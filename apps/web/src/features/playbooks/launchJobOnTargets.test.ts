import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ConnectionTarget } from "@/shared/api/types";
import { ApiError } from "@/shared/api/client";
import { testUuid } from "@/test/ids";
import { launchJobOnTargets } from "./launchJobOnTargets";

const api = vi.hoisted(() => ({ createPlaybookTargetSet: vi.fn(), launchJobTemplate: vi.fn() }));
vi.mock("./api", () => api);

const targets: ConnectionTarget[] = [
  { id: testUuid("00000000-0000-4000-8000-000000000001"), deviceId: testUuid("first-device"), name: "First host", targetKind: "device", status: "active" },
  { id: testUuid("00000000-0000-4000-8000-000000000002"), vmId: testUuid("second-vm"), name: "Second host", targetKind: "virtual_machine", status: "active" },
];

describe("job launch on fixed hosts", () => {
  beforeEach(() => vi.resetAllMocks());

  it("passes parameter overrides without broadening the selected targets", async () => {
    api.launchJobTemplate.mockResolvedValue({ run_group: { id: "run" } });
    const extraVars = { count: 0, enabled: false, name: "", nullable: null };
    await launchJobOnTargets("job", [targets[0]!], "connections.host_assignment", extraVars);
    expect(api.launchJobTemplate).toHaveBeenCalledWith("job", {
      target_ids: [targets[0]!.id], metadata: { source: "connections.host_assignment", target_count: 1 }, overrides: { extra_vars: extraVars },
    });
  });

  it("narrows the selected job to explicit target IDs without creating a target set", async () => {
    api.launchJobTemplate.mockResolvedValue({ run_group: { id: "run" } });
    await expect(launchJobOnTargets("job", targets, "connections.bulk_launch")).resolves.toEqual({ run_group: { id: "run" } });
    expect(api.createPlaybookTargetSet).not.toHaveBeenCalled();
    expect(api.launchJobTemplate).toHaveBeenCalledWith("job", {
      target_ids: targets.map((target) => target.id),
      metadata: { source: "connections.bulk_launch", target_count: 2 },
    });
    expect(api.launchJobTemplate).toHaveBeenCalledTimes(1);
  });

  it.each([
    "Selected target is outside the resolved template target set",
    "Manual target selection requires managed inventory",
  ])("preserves the backend constraint without fallback: %s", async (message) => {
    const error = new ApiError({ status: 400, code: "VALIDATION_ERROR", message });
    api.launchJobTemplate.mockRejectedValue(error);
    await expect(launchJobOnTargets("job", targets, "connections.bulk_launch")).rejects.toBe(error);
    expect(api.launchJobTemplate).toHaveBeenCalledTimes(1);
    expect(api.launchJobTemplate).toHaveBeenCalledWith("job", expect.objectContaining({ target_ids: targets.map((target) => target.id) }));
    expect(api.createPlaybookTargetSet).not.toHaveBeenCalled();
  });

  it("does not broaden an empty selection or launch without a job", async () => {
    await expect(launchJobOnTargets("job", [], "connections.bulk_launch")).rejects.toThrow();
    await expect(launchJobOnTargets("", targets, "connections.bulk_launch")).rejects.toThrow();
    expect(api.createPlaybookTargetSet).not.toHaveBeenCalled();
    expect(api.launchJobTemplate).not.toHaveBeenCalled();
  });

  it("rejects duplicate, nil and malformed target IDs rather than dropping them", async () => {
    await expect(launchJobOnTargets("job", [targets[0]!, targets[0]!], "connections.bulk_launch")).rejects.toThrow();
    for (const id of ["00000000-0000-0000-0000-000000000000", "not-a-target-id"]) {
      await expect(launchJobOnTargets("job", [{ ...targets[0]!, id: id as ConnectionTarget["id"] }], "connections.bulk_launch")).rejects.toThrow();
    }
    expect(api.launchJobTemplate).not.toHaveBeenCalled();
    expect(api.createPlaybookTargetSet).not.toHaveBeenCalled();
  });
});
