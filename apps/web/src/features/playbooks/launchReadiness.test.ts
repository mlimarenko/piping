import { describe, expect, it } from "vitest";
import type { AutomationSecret } from "@/features/automation-secrets/api";
import type { PlaybookTargetPreviewItem, PlaybookTargetSetPreview } from "./api";
import {
  evaluateLaunchReadiness,
  LAUNCH_TARGET_LIMIT,
  LAUNCH_TARGET_SAMPLE,
  unavailableSecretCount,
  type LaunchReadinessInput,
} from "./launchReadiness";

function host(over: Partial<PlaybookTargetPreviewItem> = {}): PlaybookTargetPreviewItem {
  return { name: "Alpha", address: "10.0.0.1", address_bound: true, source_id: "source-1", source_kind: "device", target_id: "target-1", target_kind: "connection_target", ...over };
}

function preview(over: Partial<PlaybookTargetSetPreview> = {}): PlaybookTargetSetPreview {
  return { items: [host()], total: 1, limit: LAUNCH_TARGET_LIMIT, truncated: false, target_set: { id: "set", name: "Edge routers", mode: "dynamic", entity_kind: "connection_target", selector: {}, created_at: "", updated_at: "" }, ...over };
}

const secret = (over: Partial<AutomationSecret> = {}): AutomationSecret => ({ id: "secret-1", key: "API_TOKEN", has_value: true, is_masked: true, usage_count: 1, created_at: "", ...over });

const base: LaunchReadinessInput = {
  jobReady: true,
  canLaunch: true,
  targetSetId: "set",
  adHocTargets: false,
  fixedTargetCount: 0,
  preview: { status: "ready", data: preview() },
  secretRefs: {},
  secrets: { status: "ready", data: [] },
};

const evaluate = (over: Partial<LaunchReadinessInput> = {}) => evaluateLaunchReadiness({ ...base, ...over });

describe("launch readiness", () => {
  it("is launchable with a resolved composition", () => {
    const readiness = evaluate();
    expect(readiness.blockers).toEqual([]);
    expect(readiness.launchable).toBe(true);
    expect(readiness.composition).toMatchObject({ targetSetId: "set", name: "Edge routers", mode: "dynamic", total: 1 });
    expect(readiness.composition?.sample).toHaveLength(1);
  });

  it("keeps the first hosts as a bounded sample of the composition", () => {
    const items = Array.from({ length: 8 }, (_, index) => host({ name: `host-${index}`, target_id: `target-${index}` }));
    const readiness = evaluate({ preview: { status: "ready", data: preview({ items, total: 8 }) } });
    expect(readiness.composition?.sample).toHaveLength(LAUNCH_TARGET_SAMPLE);
    expect(readiness.composition?.sample[0]?.name).toBe("host-0");
    expect(readiness.composition?.total).toBe(8);
  });

  it("blocks a set that resolves to no hosts", () => {
    const readiness = evaluate({ preview: { status: "ready", data: preview({ items: [], total: 0 }) } });
    expect(readiness.blockers.map((blocker) => blocker.code)).toContain("empty-target-set");
    expect(readiness.launchable).toBe(false);
  });

  it("blocks a composition above the UI limit and keeps the real count", () => {
    const readiness = evaluate({ preview: { status: "ready", data: preview({ total: LAUNCH_TARGET_LIMIT + 1 }) } });
    expect(readiness.blockers).toContainEqual({ code: "target-set-over-limit", count: LAUNCH_TARGET_LIMIT + 1 });
    expect(readiness.launchable).toBe(false);
  });

  it.each([
    ["a null address", host({ address: null })],
    ["an unbound address", host({ address: "10.0.0.1", address_bound: false })],
    ["a blank address", host({ address: "  " })],
  ])("blocks %s", (_label, item) => {
    const readiness = evaluate({ preview: { status: "ready", data: preview({ items: [host(), item], total: 2 }) } });
    expect(readiness.blockers).toContainEqual({ code: "unbound-address", count: 1 });
    expect(readiness.launchable).toBe(false);
  });

  it("blocks without execute permission", () => {
    const readiness = evaluate({ canLaunch: false });
    expect(readiness.blockers.map((blocker) => blocker.code)).toEqual(["no-permission"]);
    expect(readiness.launchable).toBe(false);
  });

  it("never reports a missing or failed composition as zero hosts", () => {
    const missing = evaluate({ preview: undefined });
    expect(missing.blockers.map((blocker) => blocker.code)).toEqual(["target-set-loading"]);
    expect(missing.composition).toBeNull();
    const failed = evaluate({ preview: { status: "failed" } });
    expect(failed.blockers.map((blocker) => blocker.code)).toEqual(["target-set-failed"]);
    expect(failed.composition).toBeNull();
    expect(failed.launchable).toBe(false);
  });

  it("has no composition gate for native inventory", () => {
    const readiness = evaluate({ targetSetId: null, preview: undefined });
    expect(readiness.blockers).toEqual([]);
    expect(readiness.composition).toBeNull();
    expect(readiness.launchable).toBe(true);
  });

  it("checks the ad-hoc selection against the same UI limit", () => {
    expect(evaluate({ adHocTargets: true, fixedTargetCount: 0, preview: undefined }).blockers.map((blocker) => blocker.code)).toEqual(["empty-selection"]);
    expect(evaluate({ adHocTargets: true, fixedTargetCount: LAUNCH_TARGET_LIMIT + 1, preview: undefined }).blockers).toContainEqual({ code: "target-set-over-limit", count: LAUNCH_TARGET_LIMIT + 1 });
    expect(evaluate({ adHocTargets: true, fixedTargetCount: 2, preview: undefined }).launchable).toBe(true);
    expect(evaluate({ adHocTargets: true, targetSetId: null, fixedTargetCount: 2 }).blockers.map((blocker) => blocker.code)).toEqual(["fixed-targets-unsupported"]);
  });

  it("gates the launch on every referenced secret", () => {
    const refs = { API_TOKEN: "secret-1", SSH_KEY: "secret-2" };
    expect(evaluate({ secretRefs: refs, secrets: { status: "ready", data: [secret()] } }).blockers).toContainEqual({ code: "missing-secrets", count: 1 });
    expect(evaluate({ secretRefs: refs, secrets: { status: "ready", data: [secret(), secret({ id: "secret-2", key: "SSH_KEY", has_value: false })] } }).blockers).toContainEqual({ code: "missing-secrets", count: 1 });
    expect(evaluate({ secretRefs: refs, secrets: { status: "ready", data: [secret(), secret({ id: "secret-2", key: "SSH_KEY" })] } }).launchable).toBe(true);
    expect(evaluate({ secretRefs: refs, secrets: undefined }).blockers.map((blocker) => blocker.code)).toEqual(["secrets-loading"]);
    expect(evaluate({ secretRefs: refs, secrets: { status: "failed" } }).blockers.map((blocker) => blocker.code)).toEqual(["secrets-failed"]);
  });

  it("does not require the secret list when the job declares none", () => {
    expect(evaluate({ secrets: undefined }).launchable).toBe(true);
    expect(unavailableSecretCount({}, [])).toBe(0);
  });

  it("is not launchable before the job, entrypoint or project resolve", () => {
    const readiness = evaluate({ jobReady: false });
    expect(readiness.blockers).toEqual([]);
    expect(readiness.launchable).toBe(false);
  });
});
