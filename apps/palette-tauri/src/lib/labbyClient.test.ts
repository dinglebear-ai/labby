import { beforeEach, describe, expect, it, vi } from "vitest";

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));

vi.mock("./invoke", () => ({
  invoke: invokeMock,
}));

import { executeLauncherEntry, fetchLauncherCatalog, fetchLauncherSchema } from "./labbyClient";

describe("launcher client wrappers", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("fetchLauncherCatalog returns decoded entries", async () => {
    invokeMock.mockResolvedValueOnce({
      ok: true,
      status: 200,
      payload: {
        fingerprint: "fp",
        entries: [{ kind: "mcpTool", id: "mcp:alpha::ping", label: "ping" }],
      },
    });

    const result = await fetchLauncherCatalog("etag-1");

    expect(invokeMock).toHaveBeenCalledWith("fetch_launcher_catalog", { etag: "etag-1" });
    expect(result).toEqual({
      notModified: false,
      catalog: {
        fingerprint: "fp",
        entries: [{ kind: "mcpTool", id: "mcp:alpha::ping", label: "ping" }],
      },
    });
  });

  it("executeLauncherEntry posts id params and options", async () => {
    invokeMock.mockResolvedValueOnce({ ok: true, status: 200, payload: { value: 1 } });

    const result = await executeLauncherEntry("mcp:alpha::ping", { q: "hello" }, { confirmDestructive: true });

    expect(invokeMock).toHaveBeenCalledWith("execute_launcher_entry", {
      request: {
        id: "mcp:alpha::ping",
        params: { q: "hello" },
        confirmDestructive: true,
      },
    });
    expect(result).toEqual({
      ok: true,
      status: 200,
      path: "/v1/palette/execute",
      method: "POST",
      payload: { value: 1 },
    });
  });

  it("fetchLauncherSchema requests schema by launcher id", async () => {
    invokeMock.mockResolvedValueOnce({
      ok: true,
      status: 200,
      payload: { id: "mcp:alpha::ping", inputSchema: { type: "object" } },
    });

    const result = await fetchLauncherSchema("mcp:alpha::ping");

    expect(invokeMock).toHaveBeenCalledWith("fetch_launcher_schema", { id: "mcp:alpha::ping" });
    expect(result).toEqual({ id: "mcp:alpha::ping", inputSchema: { type: "object" } });
  });

  it("HTTP errors return stable payloads rather than throwing", async () => {
    invokeMock.mockResolvedValueOnce({
      ok: false,
      status: 422,
      payload: { kind: "invalid_param", message: "bad params" },
    });

    await expect(fetchLauncherCatalog()).resolves.toEqual({
      ok: false,
      status: 422,
      path: "/v1/palette/catalog",
      method: "GET",
      payload: { kind: "invalid_param", message: "bad params" },
    });
  });
});
