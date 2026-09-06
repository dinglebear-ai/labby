import { beforeEach, describe, expect, it, vi } from "vitest";

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));

vi.mock("./invoke", () => ({
  invoke: invokeMock,
}));

import { executeLauncherEntry, fetchLauncherCatalog, fetchLauncherSchema } from "./labbyClient";

const executableEntry = {
  id: "mcp:alpha::ping",
  contractHash: "a".repeat(64),
};

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
        entries: [
          {
            kind: "mcpTool",
            id: "mcp:alpha::ping",
            label: "ping",
            description: "Ping alpha",
            source: "alpha",
            destructive: false,
            upstream: "alpha",
            tool: "ping",
          },
        ],
      },
    });

    const result = await fetchLauncherCatalog("", "etag-1");

    expect(invokeMock).toHaveBeenCalledWith("fetch_launcher_catalog", {
      query: "",
      etag: "etag-1",
    });
    expect(result).toEqual({
      notModified: false,
      catalog: {
        fingerprint: "fp",
        entries: [
          {
            kind: "mcpTool",
            id: "mcp:alpha::ping",
            label: "ping",
            description: "Ping alpha",
            source: "alpha",
            destructive: false,
            upstream: "alpha",
            tool: "ping",
          },
        ],
      },
    });
  });

  it("executeLauncherEntry posts id params and options", async () => {
    invokeMock.mockResolvedValueOnce({ ok: true, status: 200, payload: { value: 1 } });

    const result = await executeLauncherEntry(
      executableEntry,
      { q: "hello" },
      { confirmDestructive: true },
    );

    expect(invokeMock).toHaveBeenCalledWith("execute_launcher_entry", {
      request: {
        id: "mcp:alpha::ping",
        params: { q: "hello" },
        confirmDestructive: true,
        expectedContractHash: "a".repeat(64),
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

  it("refuses to invoke the bridge when the selected entry has no contract hash", async () => {
    await expect(
      executeLauncherEntry({ id: "mcp:alpha::ping", contractHash: "" }, { q: "hello" }),
    ).rejects.toThrow("current contract hash");

    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("returns contract_changed without retrying a stale hash", async () => {
    invokeMock.mockResolvedValueOnce({
      ok: false,
      status: 409,
      payload: { kind: "contract_changed", message: "review the current contract" },
    });

    const result = await executeLauncherEntry(executableEntry, { q: "hello" });

    expect(result).toMatchObject({ ok: false, status: 409 });
    expect(invokeMock).toHaveBeenCalledTimes(1);
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

    await expect(fetchLauncherCatalog("needle")).resolves.toEqual({
      ok: false,
      status: 422,
      path: "/v1/palette/search",
      method: "GET",
      payload: { kind: "invalid_param", message: "bad params" },
    });
    expect(invokeMock).toHaveBeenCalledWith("fetch_launcher_catalog", {
      query: "needle",
      etag: null,
    });
  });

  it.each([
    null,
    { fingerprint: "fp" },
    { fingerprint: "fp", entries: "not-an-array" },
    { fingerprint: "fp", entries: [], truncated: "yes" },
    { fingerprint: "fp", entries: [{ kind: "mcpTool", id: "mcp:a::b" }] },
    {
      fingerprint: "fp",
      entries: [
        {
          kind: "unknown",
          id: "unknown:a",
          label: "a",
          description: "",
          source: "a",
          destructive: false,
        },
      ],
    },
  ])("rejects malformed successful launcher payload %#", async (payload) => {
    invokeMock.mockResolvedValueOnce({ ok: true, status: 200, payload });

    await expect(fetchLauncherCatalog()).rejects.toThrow("invalid shape");
  });
});
