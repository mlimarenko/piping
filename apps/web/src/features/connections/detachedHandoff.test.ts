import { describe, expect, it } from "vitest";

import { normalizeDetachedHandoff } from "./detachedHandoff";

describe("normalizeDetachedHandoff", () => {
  it("accepts a cctv handoff with an empty transport", () => {
    const handoff = {
      sessionId: "cctv:target-1",
      hostId: "target-1",
      hostName: "Регистратор 1-1",
      protocol: "cctv",
      websocketUrl: "",
      token: "",
    };
    expect(normalizeDetachedHandoff(handoff)).toEqual(handoff);
  });

  it("still rejects a proxied-session handoff with an empty transport", () => {
    expect(
      normalizeDetachedHandoff({
        sessionId: "session-1",
        hostId: "host-1",
        hostName: "host",
        protocol: "ssh",
        websocketUrl: "",
        token: "",
      }),
    ).toBeNull();
  });

  it("rejects unknown protocols", () => {
    expect(
      normalizeDetachedHandoff({
        sessionId: "s",
        hostId: "h",
        hostName: "n",
        protocol: "video",
        websocketUrl: "",
        token: "",
      }),
    ).toBeNull();
  });
});
