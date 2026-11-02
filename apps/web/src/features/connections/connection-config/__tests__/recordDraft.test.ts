import { describe, expect, it } from "vitest";

import { asParams } from "../api";
import {
  DEFAULT_ENDPOINT_HOST,
  draftFromRecord,
  emptyDraft,
  paramsForProtocolChange,
} from "../recordDraft";
import type { RecordDraft } from "../recordDraft";
import type { ConnConfRecord } from "../api";

// ── helpers ──────────────────────────────────────────────

function makeRecord(overrides: Partial<ConnConfRecord> = {}): ConnConfRecord {
  return {
    id: "aaaaaaaa-0000-0000-0000-000000000000",
    name: "prod-ssh",
    description: "test record",
    protocol: "ssh",
    base_record_id: null,
    enabled: true,
    params: {},
    custom_field_data: null,
    created_at: "2024-01-01T00:00:00Z",
    updated_at: "2024-01-01T00:00:00Z",
    ...overrides,
  };
}

// ── emptyDraft ───────────────────────────────────────────

describe("emptyDraft", () => {
  it("returns ssh defaults when no protocol supplied", () => {
    const draft = emptyDraft();
    expect(draft.protocol).toBe("ssh");
    expect(draft.name).toBe("");
    expect(draft.description).toBe("");
    expect(draft.baseRecordId).toBe("");
    expect(draft.enabled).toBe(true);
    expect(draft.params).toEqual({ host: DEFAULT_ENDPOINT_HOST });
  });

  it("honours an explicit protocol", () => {
    expect(emptyDraft("rdp").protocol).toBe("rdp");
    expect(emptyDraft("vnc").protocol).toBe("vnc");
  });

  it("preselects primary IPv4 for session protocols and leaves app targets explicit", () => {
    expect(emptyDraft("ssh").params).toEqual({ host: DEFAULT_ENDPOINT_HOST });
    expect(emptyDraft("rdp").params).toEqual({ host: DEFAULT_ENDPOINT_HOST });
    expect(emptyDraft("vnc").params).toEqual({ host: DEFAULT_ENDPOINT_HOST });
    expect(emptyDraft("app").params).toEqual({});
  });

  it("seeds a video draft with the endpoint host and nothing else", () => {
    expect(emptyDraft("cctv").protocol).toBe("cctv");
    expect(emptyDraft("cctv").params).toEqual({ host: DEFAULT_ENDPOINT_HOST });
  });
});

// ── draftFromRecord ──────────────────────────────────────

describe("draftFromRecord", () => {
  it("maps a minimal ssh record to a draft", () => {
    const record = makeRecord();
    const draft = draftFromRecord(record);
    expect(draft.name).toBe("prod-ssh");
    expect(draft.description).toBe("test record");
    expect(draft.protocol).toBe("ssh");
    expect(draft.baseRecordId).toBe("");
    expect(draft.enabled).toBe(true);
    expect(draft.params).toEqual({});
  });

  it("falls back to ssh for unknown protocols", () => {
    // The function coerces anything that isn't rdp/vnc to ssh.
    // Pass an unknown protocol via a wider string cast so TS doesn't narrow it.
    const record = makeRecord({ protocol: "telnet" });
    const draft = draftFromRecord(record);
    expect(draft.protocol).toBe("ssh");
  });

  it("preserves rdp protocol", () => {
    const record = makeRecord({ protocol: "rdp" });
    expect(draftFromRecord(record).protocol).toBe("rdp");
  });

  it("preserves vnc protocol", () => {
    const record = makeRecord({ protocol: "vnc" });
    expect(draftFromRecord(record).protocol).toBe("vnc");
  });

  it("maps base_record_id to baseRecordId", () => {
    const parentId = "bbbbbbbb-0000-0000-0000-000000000000";
    const draft = draftFromRecord(makeRecord({ base_record_id: parentId }));
    expect(draft.baseRecordId).toBe(parentId);
  });

  it("converts null base_record_id to empty string", () => {
    const draft = draftFromRecord(makeRecord({ base_record_id: null }));
    expect(draft.baseRecordId).toBe("");
  });

  it("round-trips a record's params through asParams", () => {
    const params = { host: "10.0.0.1", port: 22, credentials: { username: "root" } };
    const draft = draftFromRecord(makeRecord({ params }));
    expect(draft.params).toEqual(params);
  });

  it("produces empty params when record params is not an object", () => {
    // asParams guards against non-object values
    const draft = draftFromRecord(makeRecord({ params: null as unknown as object }));
    expect(draft.params).toEqual({});
  });

  it("round-trip: draft shape that would be sent on create matches original fields", () => {
    const record = makeRecord({
      name: "dc1-rdp",
      description: "datacenter rdp",
      protocol: "rdp",
      base_record_id: "cccccccc-0000-0000-0000-000000000000",
      enabled: false,
      params: { host: "192.168.1.100", port: 3389 },
    });
    const draft: RecordDraft = draftFromRecord(record);

    // Simulate the payload assembly from ConnectionConfig.submit()
    const payload = {
      name: draft.name.trim(),
      description: draft.description.trim() || null,
      protocol: draft.protocol,
      base_record_id: draft.baseRecordId || null,
      enabled: draft.enabled,
      params: draft.params,
    };

    expect(payload.name).toBe("dc1-rdp");
    expect(payload.protocol).toBe("rdp");
    expect(payload.base_record_id).toBe("cccccccc-0000-0000-0000-000000000000");
    expect(payload.enabled).toBe(false);
    expect(payload.params).toEqual({ host: "192.168.1.100", port: 3389 });
  });
});

// ── asParams (used by draftFromRecord) ───────────────────

describe("asParams", () => {
  it("passes through a valid params object unchanged", () => {
    const params = { host: "host.example.com", port: 22 };
    expect(asParams(params)).toBe(params);
  });

  it("returns empty object for null", () => {
    expect(asParams(null)).toEqual({});
  });

  it("returns empty object for a non-object primitive", () => {
    expect(asParams("string")).toEqual({});
    expect(asParams(42)).toEqual({});
  });

  it("returns empty object for arrays", () => {
    expect(asParams(["ssh"])).toEqual({});
  });
});

// ── paramsForProtocolChange ──────────────────────────────

describe("paramsForProtocolChange", () => {
  it("keeps safe shared fields and drops credentials when switching session protocols", () => {
    const next = paramsForProtocolChange("rdp", {
      host: "{{primary_ip4}}",
      port: 22,
      timeout_secs: 30,
      credentials: {
        username: "root",
        password: "secret",
        private_key: "key",
      },
      options: { width: 1920 },
      ui: { label: "Admin", color: "#ffaa00" },
    });

    expect(next).toEqual({
      host: "{{primary_ip4}}",
      timeout_secs: 30,
      ui: { label: "Admin", color: "#ffaa00" },
    });
  });

  it("drops session fields when switching to app", () => {
    const next = paramsForProtocolChange("app", {
      host: "{{primary_ip4}}",
      port: 3389,
      credentials: { username: "operator" },
      url: "https://{{primary_ip4}}",
      ui: { label: "Console" },
    });

    expect(next).toEqual({
      url: "https://{{primary_ip4}}",
      ui: { label: "Console" },
    });
  });

  it("uses primary IPv4 when switching from app to a session protocol without a host", () => {
    const next = paramsForProtocolChange("ssh", {
      url: "https://{{primary_ip4}}",
      ui: { label: "Console" },
    });

    expect(next).toEqual({
      host: DEFAULT_ENDPOINT_HOST,
      ui: { label: "Console" },
    });
  });

  it("drops port and timeout_secs when switching to cctv, keeping host, credentials and ui", () => {
    const next = paramsForProtocolChange("cctv", {
      host: "10.0.0.9",
      port: 34567,
      timeout_secs: 30,
      credentials: { username: "admin", password: "secret" },
      ui: { label: "Cam" },
    });

    expect(next).toEqual({
      host: "10.0.0.9",
      credentials: { username: "admin", password: "secret" },
      ui: { label: "Cam" },
    });
    // Video rejects both of these on the wire.
    expect(next).not.toHaveProperty("port");
    expect(next).not.toHaveProperty("timeout_secs");
  });

  it("uses primary IPv4 when switching to cctv without a host", () => {
    const next = paramsForProtocolChange("cctv", { port: 3389 });
    expect(next).toEqual({ host: DEFAULT_ENDPOINT_HOST });
  });
});

// ── submit validation (name required guard) ───────────────

describe("submit validation", () => {
  it("empty name triggers nameRequired", () => {
    // The validation in ConnectionConfig.submit() is:  !draft.name.trim()
    // Test the predicate directly against draft values.
    const draft = emptyDraft();
    expect(!draft.name.trim()).toBe(true);
  });

  it("whitespace-only name is considered empty", () => {
    const draft = { ...emptyDraft(), name: "   " };
    expect(!draft.name.trim()).toBe(true);
  });

  it("non-empty name passes validation", () => {
    const draft = { ...emptyDraft(), name: "prod" };
    expect(!draft.name.trim()).toBe(false);
  });
});

// ── secret-ref shape detection ───────────────────────────

describe("secret-ref heuristic (looksLikeSecretRef)", () => {
  // The UUID regex is private to RecordForm.tsx. We test the *effect*:
  // a UUID-shaped password string should come through the draft unchanged
  // (it's stored as a bare string; the secret binding is separate state).
  const UUID = "12345678-1234-1234-1234-123456789abc";

  it("UUID value is preserved as-is in params.credentials.password", () => {
    const record = makeRecord({
      params: { credentials: { password: UUID } },
    });
    const draft = draftFromRecord(record);
    expect(draft.params.credentials?.password).toBe(UUID);
  });

  it("literal password is preserved as-is", () => {
    const record = makeRecord({
      params: { credentials: { password: "hunter2" } },
    });
    const draft = draftFromRecord(record);
    expect(draft.params.credentials?.password).toBe("hunter2");
  });

  it("private_key UUID reference is preserved", () => {
    const record = makeRecord({
      params: { credentials: { private_key: UUID } },
    });
    const draft = draftFromRecord(record);
    expect(draft.params.credentials?.private_key).toBe(UUID);
  });
});
