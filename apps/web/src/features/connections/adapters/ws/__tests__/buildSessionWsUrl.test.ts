import { describe, expect, it } from "vitest";

import { buildSessionWsUrl, isSessionWsConnectable } from "../urls";

describe("buildSessionWsUrl", () => {
  it("prefixes a relative path with wss:// using the current host", () => {
    const url = buildSessionWsUrl("/api/v1/ws/ssh/abc", "tok-123");
    const parsed = new URL(url);
    expect(parsed.protocol).toMatch(/^wss?:$/);
    expect(parsed.host).toBe(window.location.host);
    expect(parsed.pathname).toBe("/api/v1/ws/ssh/abc");
    expect(parsed.searchParams.get("token")).toBe("tok-123");
  });

  it("preserves an existing absolute ws(s) URL", () => {
    const url = buildSessionWsUrl("wss://edge.example/api/v1/ws/rdp/1", "secret");
    const parsed = new URL(url);
    expect(parsed.protocol).toBe("wss:");
    expect(parsed.host).toBe("edge.example");
    expect(parsed.pathname).toBe("/api/v1/ws/rdp/1");
    expect(parsed.searchParams.get("token")).toBe("secret");
  });

  it("preserves existing query params and adds token without clobbering them", () => {
    const url = buildSessionWsUrl("wss://edge.example/api/v1/ws/vnc/1?mode=binary", "tok");
    const parsed = new URL(url);
    expect(parsed.searchParams.get("mode")).toBe("binary");
    expect(parsed.searchParams.get("token")).toBe("tok");
  });

  it("replaces a pre-existing token query param instead of duplicating it", () => {
    const url = buildSessionWsUrl("wss://edge/ws?token=stale", "fresh");
    const parsed = new URL(url);
    expect(parsed.searchParams.getAll("token")).toEqual(["fresh"]);
  });
});

describe("isSessionWsConnectable", () => {
  it("accepts a real absolute relay URL with a non-empty token", () => {
    expect(isSessionWsConnectable("wss://app.piping.space/api/v1/ws/ssh/abc", "tok")).toBe(true);
  });

  it("accepts a relative relay path with a token", () => {
    expect(isSessionWsConnectable("/api/v1/ws/ssh/abc", "tok")).toBe(true);
  });

  it("accepts the demo relay URL", () => {
    expect(isSessionWsConnectable("wss://demo.invalid/relay/ssh/linux-host", "demo-token")).toBe(true);
  });

  it("rejects a host-only URL (no relay path) — the bogus wss://host/?token= case", () => {
    expect(isSessionWsConnectable("wss://app.piping.space", "tok")).toBe(false);
    expect(isSessionWsConnectable("wss://app.piping.space/", "tok")).toBe(false);
  });

  it("rejects an empty or whitespace-only token even with a valid path", () => {
    expect(isSessionWsConnectable("wss://app.piping.space/api/v1/ws/ssh/abc", "")).toBe(false);
    expect(isSessionWsConnectable("wss://app.piping.space/api/v1/ws/ssh/abc", "   ")).toBe(false);
  });

  it("rejects an empty URL", () => {
    expect(isSessionWsConnectable("", "tok")).toBe(false);
  });
});
