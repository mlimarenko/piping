import {
  CONNECTIONS_HOST_PARAM,
  CONNECTIONS_PATH,
  connectionsHostHref,
  connectionsHostUrl,
} from "./workspaceLinks";

describe("workspaceLinks", () => {
  const id = "019ed60c-f3d8-7952-9643-478d94a47fbb";

  it("builds a relative href with the host param", () => {
    expect(connectionsHostHref(id)).toBe(`${CONNECTIONS_PATH}?${CONNECTIONS_HOST_PARAM}=${id}`);
  });

  it("round-trips through URLSearchParams (the param the workspace reads)", () => {
    const href = connectionsHostHref(id);
    const search = new URLSearchParams(href.split("?")[1]);
    expect(search.get(CONNECTIONS_HOST_PARAM)).toBe(id);
  });

  it("encodes ids that need escaping", () => {
    const href = connectionsHostHref("a/b c");
    const search = new URLSearchParams(href.split("?")[1]);
    expect(search.get(CONNECTIONS_HOST_PARAM)).toBe("a/b c");
  });

  it("builds an absolute, shareable URL from the current origin", () => {
    const url = connectionsHostUrl(id);
    expect(url).toBe(`${window.location.origin}${connectionsHostHref(id)}`);
    expect(url).toContain(`${CONNECTIONS_PATH}?${CONNECTIONS_HOST_PARAM}=${id}`);
  });
});
