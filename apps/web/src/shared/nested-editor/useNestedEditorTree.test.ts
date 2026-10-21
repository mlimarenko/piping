import { act, renderHook, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import {
  useNestedEditorTree,
  type RelationDescriptor,
} from "./useNestedEditorTree";

/** A minimal row shape the descriptor mock returns. */
interface Row {
  id: string;
  [k: string]: unknown;
}

/**
 * Build a mock descriptor with vi.fn CRUD. `childRelation` is wired through so
 * the temp-id-resolution / bottom-up-delete walks are exercised end to end.
 */
function makeDescriptor(
  key: string,
  parentFkField: string,
  overrides: Partial<RelationDescriptor<Row>> = {},
): RelationDescriptor<Row> {
  let createSeq = 0;
  const rowName = (row: Row): string => (typeof row.name === "string" ? row.name : "");
  return {
    key,
    title: `t.${key}`,
    parentFkField,
    pageLimit: 1000,
    listByParent: vi.fn(() => Promise.resolve({ rows: [] as Row[], total: 0, loaded: 0 })),
    create: vi.fn((payload: Record<string, unknown>) => {
      createSeq += 1;
      const id = `${key}-real-${createSeq}`;
      return Promise.resolve({ id, raw: { id, ...payload } });
    }),
    update: vi.fn((realId: string, patch: Record<string, unknown>) =>
      Promise.resolve({ id: realId, ...patch }),
    ),
    remove: vi.fn(() => Promise.resolve()),
    buildDraft: () => ({ name: "" }),
    toDraft: (row: Row) => ({ name: rowName(row), __id: row.id }),
    toPayload: (draft, parentRealId) => ({
      ...draft,
      ...(parentRealId ? { [parentFkField]: parentRealId } : {}),
    }),
    validate: () => ({}),
    ...overrides,
  };
}

const ROOT_ID = "device-1";

describe("useNestedEditorTree", () => {
  it("creates top-down and injects resolved parent ids into children (temp-id resolution)", async () => {
    const ipDesc = makeDescriptor("ips", "interfaceId");
    const ifaceDesc = makeDescriptor("interfaces", "deviceId", {
      childRelation: ipDesc as unknown as RelationDescriptor,
    });

    const { result } = renderHook(() =>
      useNestedEditorTree({ rootRealId: ROOT_ID, relations: [ifaceDesc] }),
    );

    let ifaceLocalId = "";
    act(() => {
      ifaceLocalId = result.current.addRow("interfaces");
    });
    act(() => {
      result.current.patchDraft(ifaceLocalId, "name", "eth0");
    });
    let ipLocalId = "";
    act(() => {
      ipLocalId = result.current.addRow("ips", ifaceLocalId);
    });
    act(() => {
      result.current.patchDraft(ipLocalId, "name", "10.0.0.1/24");
    });

    let outcome: Awaited<ReturnType<typeof result.current.save>> | undefined;
    await act(async () => {
      outcome = await result.current.save();
    });

    expect(outcome?.ok).toBe(true);
    expect(outcome?.persisted).toHaveLength(2);

    // Parent created first, with no parent FK injected (it is a root level).
    expect(ifaceDesc.create).toHaveBeenCalledTimes(1);
    const ifacePayload = (ifaceDesc.create as ReturnType<typeof vi.fn>).mock.calls[0]![0] as Record<string, unknown>;
    expect(ifacePayload.deviceId).toBe(ROOT_ID);

    // Child created with the parent's resolved REAL id injected as its FK.
    expect(ipDesc.create).toHaveBeenCalledTimes(1);
    const ipPayload = (ipDesc.create as ReturnType<typeof vi.fn>).mock.calls[0]![0] as Record<string, unknown>;
    expect(ipPayload.interfaceId).toBe("interfaces-real-1");

    // Nodes ended clean + real.
    expect(result.current.nodes[ifaceLocalId]?.status).toBe("clean");
    expect(result.current.nodes[ifaceLocalId]?.realId).toBe("interfaces-real-1");
    expect(result.current.nodes[ipLocalId]?.status).toBe("clean");
    expect(result.current.nodes[ipLocalId]?.realId).toBe("ips-real-1");
    expect(result.current.isDirty).toBe(false);
  });

  it("deletes bottom-up: a child IP is removed before its interface", async () => {
    const removeOrder: string[] = [];
    const ipDesc = makeDescriptor("ips", "interfaceId", {
      // Existing rows: one IP under the interface.
      remove: vi.fn((realId: string) => {
        removeOrder.push(`ip:${realId}`);
        return Promise.resolve();
      }),
    });
    const ifaceDesc = makeDescriptor("interfaces", "deviceId", {
      listByParent: vi.fn(() =>
        Promise.resolve({ rows: [{ id: "iface-1", name: "eth0" }], total: 1, loaded: 1 }),
      ),
      remove: vi.fn((realId: string) => {
        removeOrder.push(`iface:${realId}`);
        return Promise.resolve();
      }),
      childRelation: ipDesc as unknown as RelationDescriptor,
    });
    ipDesc.listByParent = vi.fn(() =>
      Promise.resolve({ rows: [{ id: "ip-1", name: "10.0.0.1/24" }], total: 1, loaded: 1 }),
    );

    const { result } = renderHook(() =>
      useNestedEditorTree({ rootRealId: ROOT_ID, relations: [ifaceDesc] }),
    );

    // Load the root relation, then the interface's children.
    act(() => {
      (result.current as unknown as { loadRootRelation: (k: string) => void }).loadRootRelation("interfaces");
    });
    await waitFor(() =>
      expect(result.current.rowsByRelation("interfaces").length).toBe(1),
    );
    const ifaceLocalId = result.current.rowsByRelation("interfaces")[0]!;
    act(() => {
      result.current.toggleExpand(ifaceLocalId);
    });
    await waitFor(() =>
      expect(result.current.rowsByRelation("ips", ifaceLocalId).length).toBe(1),
    );

    // Delete the interface → cascades a 'deleted' status to its IP child.
    act(() => {
      result.current.markDeleted(ifaceLocalId);
    });
    const ipLocalId = result.current.rowsByRelation("ips", ifaceLocalId)[0]!;
    expect(result.current.nodes[ipLocalId]?.status).toBe("deleted");

    await act(async () => {
      await result.current.save();
    });

    // Bottom-up: the IP delete must precede its parent interface delete.
    expect(removeOrder).toEqual(["ip:ip-1", "iface:iface-1"]);
  });

  it("partial failure: a failing child persists its siblings and blocks its own uncreated descendants", async () => {
    const ipDesc = makeDescriptor("ips", "interfaceId");
    // The SECOND interface create fails; the first succeeds.
    let calls = 0;
    const ifaceDesc = makeDescriptor("interfaces", "deviceId", {
      create: vi.fn((payload: Record<string, unknown>) => {
        calls += 1;
        if (calls === 2) return Promise.reject(new Error("duplicate name"));
        const id = `iface-real-${calls}`;
        return Promise.resolve({ id, raw: { id, ...payload } });
      }),
      childRelation: ipDesc as unknown as RelationDescriptor,
    });

    const { result } = renderHook(() =>
      useNestedEditorTree({ rootRealId: ROOT_ID, relations: [ifaceDesc] }),
    );

    let a = "";
    let b = "";
    act(() => {
      a = result.current.addRow("interfaces");
    });
    act(() => {
      b = result.current.addRow("interfaces");
    });
    // The failing interface (b) has a child IP that can never get a real FK.
    let childOfB = "";
    act(() => {
      childOfB = result.current.addRow("ips", b);
    });

    let outcome: Awaited<ReturnType<typeof result.current.save>> | undefined;
    await act(async () => {
      outcome = await result.current.save();
    });

    expect(outcome?.ok).toBe(false);
    // First interface committed; second failed; its child blocked.
    expect(result.current.nodes[a]?.status).toBe("clean");
    expect(result.current.nodes[a]?.realId).toBe("iface-real-1");
    expect(result.current.nodes[b]?.status).toBe("failed");
    expect(result.current.nodes[b]?.error).toBe("duplicate name");
    expect(result.current.nodes[childOfB]?.status).toBe("blocked");

    expect(outcome?.persisted).toContain(a);
    expect(outcome?.failed.map((f) => f.nodeId)).toContain(b);
    expect(outcome?.blocked).toContain(childOfB);
    // The blocked child's create was never attempted.
    expect(ipDesc.create).not.toHaveBeenCalled();
  });

  it("retry is skip-clean idempotent: a re-save only re-issues calls for non-clean nodes", async () => {
    let calls = 0;
    const ifaceDesc = makeDescriptor("interfaces", "deviceId", {
      create: vi.fn((payload: Record<string, unknown>) => {
        calls += 1;
        // Fail the first attempt for the second row, succeed on retry.
        if (calls === 2) return Promise.reject(new Error("transient"));
        const id = `iface-real-${calls}`;
        return Promise.resolve({ id, raw: { id, ...payload } });
      }),
    });

    const { result } = renderHook(() =>
      useNestedEditorTree({ rootRealId: ROOT_ID, relations: [ifaceDesc] }),
    );

    let a = "";
    let b = "";
    act(() => {
      a = result.current.addRow("interfaces");
    });
    act(() => {
      b = result.current.addRow("interfaces");
    });

    await act(async () => {
      await result.current.save();
    });
    expect(result.current.nodes[a]?.status).toBe("clean");
    expect(result.current.nodes[b]?.status).toBe("failed");
    const createCallsAfterFirst = (ifaceDesc.create as ReturnType<typeof vi.fn>).mock.calls.length;
    expect(createCallsAfterFirst).toBe(2);

    // Re-arm the failed node and re-save: ONLY the failed node re-issues.
    act(() => {
      result.current.retryNode(b);
    });
    let outcome: Awaited<ReturnType<typeof result.current.save>> | undefined;
    await act(async () => {
      outcome = await result.current.save();
    });

    expect(outcome?.ok).toBe(true);
    // The already-clean node `a` was NOT re-created on the retry pass.
    expect((ifaceDesc.create as ReturnType<typeof vi.fn>).mock.calls.length).toBe(3);
    expect(result.current.nodes[b]?.status).toBe("clean");
    expect(result.current.nodes[b]?.realId).toBe("iface-real-3");
  });

  it("a failed CREATE re-creates on the next save (decision keys on real id, not status) even after an inline edit", async () => {
    let calls = 0;
    const ifaceDesc = makeDescriptor("interfaces", "deviceId", {
      create: vi.fn((payload: Record<string, unknown>) => {
        calls += 1;
        if (calls === 1) return Promise.reject(new Error("bad name"));
        const id = `iface-real-${calls}`;
        return Promise.resolve({ id, raw: { id, ...payload } });
      }),
    });

    const { result } = renderHook(() =>
      useNestedEditorTree({ rootRealId: ROOT_ID, relations: [ifaceDesc] }),
    );

    let a = "";
    act(() => {
      a = result.current.addRow("interfaces");
    });
    await act(async () => {
      await result.current.save();
    });
    // CREATE failed: node is 'failed' with NO real id.
    expect(result.current.nodes[a]?.status).toBe("failed");
    expect(result.current.nodes[a]?.realId).toBeUndefined();

    // Inline-edit the failed-create row (flips it to 'dirty'), then save WITHOUT
    // calling retryNode: it must re-CREATE, never attempt an update with no id.
    act(() => {
      result.current.patchDraft(a, "name", "eth0-fixed");
    });
    expect(result.current.nodes[a]?.status).toBe("dirty");

    let outcome: Awaited<ReturnType<typeof result.current.save>> | undefined;
    await act(async () => {
      outcome = await result.current.save();
    });

    expect(outcome?.ok).toBe(true);
    expect(ifaceDesc.update).not.toHaveBeenCalled();
    expect((ifaceDesc.create as ReturnType<typeof vi.fn>).mock.calls.length).toBe(2);
    expect(result.current.nodes[a]?.status).toBe("clean");
    expect(result.current.nodes[a]?.realId).toBe("iface-real-2");
  });

  it("truncation guard: a partially-loaded level is flagged truncated", async () => {
    const ifaceDesc = makeDescriptor("interfaces", "deviceId", {
      // Server reports 5 total but only returns 2 rows (page cap).
      listByParent: vi.fn(() =>
        Promise.resolve({
          rows: [
            { id: "iface-1", name: "eth0" },
            { id: "iface-2", name: "eth1" },
          ],
          total: 5,
          loaded: 2,
        }),
      ),
    });

    const { result } = renderHook(() =>
      useNestedEditorTree({ rootRealId: ROOT_ID, relations: [ifaceDesc] }),
    );

    act(() => {
      (result.current as unknown as { loadRootRelation: (k: string) => void }).loadRootRelation("interfaces");
    });
    await waitFor(() =>
      expect(result.current.rowsByRelation("interfaces").length).toBe(2),
    );

    // Every loaded sibling at the truncated level carries the truncated flag and
    // the server total, so the UI can block deletes there and render "N of M"
    // (a diff can't tell removed from unloaded).
    for (const id of result.current.rowsByRelation("interfaces")) {
      expect(result.current.nodes[id]?.truncated).toBe(true);
      expect(result.current.nodes[id]?.levelTotal).toBe(5);
    }
  });
});
