import { beforeEach, describe, expect, it, vi } from "vitest";

const client = vi.hoisted(() => ({
  DELETE: vi.fn(),
  GET: vi.fn(),
  POST: vi.fn(),
  PUT: vi.fn(),
}));

vi.mock("@/shared/api/client", () => ({
  apiClient: client,
  unwrap: (response: { data?: unknown }) => {
    if (response.data === undefined) throw new Error("Empty response body");
    return response.data;
  },
}));

import { createContext, updateContext } from "./config-contexts-api";

const storedContext = {
  id: "ctx-1",
  name: "Baseline",
  description: null,
  weight: 1000,
  is_active: true,
  data: { environment: "qa" },
  created_at: null,
  updated_at: null,
};

const fullDraft = {
  name: "Baseline",
  description: "",
  weight: 1000,
  active: true,
  data: '{"environment":"qa"}',
};

beforeEach(() => {
  Object.values(client).forEach((mock) => mock.mockReset());
});

describe("config context payload boundary", () => {
  it("posts parsed JSON object data on create", async () => {
    client.POST.mockResolvedValue({ data: storedContext });

    await createContext(fullDraft);

    expect(client.POST).toHaveBeenCalledWith("/api/v1/config-contexts", {
      body: {
        name: "Baseline",
        description: "",
        weight: 1000,
        is_active: true,
        data: { environment: "qa" },
      },
    });
  });

  it("omits weight from the serialized update so the server applies its default", async () => {
    let serializedBody = "";
    client.PUT.mockImplementation((_path: string, options: { body: unknown }) => {
      serializedBody = JSON.stringify(options.body);
      return Promise.resolve({ data: storedContext });
    });

    await updateContext("ctx-1", { name: "Baseline", data: "{}" });

    expect(JSON.parse(serializedBody)).toEqual({ name: "Baseline", data: {} });
  });

  it("keeps an explicit zero weight", async () => {
    client.PUT.mockResolvedValue({ data: { ...storedContext, weight: 0 } });

    await updateContext("ctx-1", { data: "{}", weight: 0 });

    expect(client.PUT).toHaveBeenCalledWith(
      "/api/v1/config-contexts/{id}",
      expect.objectContaining({ body: expect.objectContaining({ weight: 0 }) }),
    );
  });

  it.each([
    ["invalid JSON", "{not json"],
    ["an array", "[1,2]"],
    ["null", "null"],
    ["a number", "42"],
    ["a string", '"text"'],
    ["empty text", ""],
  ])("rejects %s before calling the API", async (_label, data) => {
    await expect(updateContext("ctx-1", { data })).rejects.toThrow(
      /Config context data must be/,
    );
    expect(client.PUT).not.toHaveBeenCalled();
  });

  it("rejects a create with invalid data before calling the API", async () => {
    await expect(createContext({ ...fullDraft, data: "[]" })).rejects.toThrow(
      /Config context data must be a JSON object/,
    );
    expect(client.POST).not.toHaveBeenCalled();
  });

  it("round-trips a canonical response with explicit weight, active flag and nested data", async () => {
    const nestedData = { piping: { connections: [{ host: "a" }] }, tags: ["x"] };
    client.POST.mockResolvedValue({
      data: {
        id: "ctx-2",
        name: "Nested",
        description: "layer",
        weight: 32767,
        is_active: false,
        data: nestedData,
        created_at: "2026-01-01T00:00:00Z",
        updated_at: null,
      },
    });

    const context = await createContext(fullDraft);

    expect(context).toEqual({
      id: "ctx-2",
      name: "Nested",
      description: "layer",
      weight: 32767,
      active: false,
      data: JSON.stringify(nestedData, null, 2),
      createdAt: "2026-01-01T00:00:00Z",
      updatedAt: undefined,
    });
  });
});
