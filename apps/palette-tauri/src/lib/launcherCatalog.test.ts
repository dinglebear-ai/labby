import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { fetchLauncherCatalogMock } = vi.hoisted(() => ({ fetchLauncherCatalogMock: vi.fn() }));

vi.mock("./labbyClient", async (importOriginal) => {
  const original = await importOriginal<typeof import("./labbyClient")>();
  return {
    ...original,
    fetchLauncherCatalog: fetchLauncherCatalogMock,
  };
});

import {
  launcherEntryMatches,
  normalizeLauncherCatalog,
  useLauncherCatalog,
} from "./launcherCatalog";

function catalog(label: string, truncated = false) {
  return {
    notModified: false as const,
    catalog: {
      fingerprint: label,
      truncated,
      entries: [
        {
          kind: "mcpTool" as const,
          id: `mcp:test::${label}`,
          contractHash: "contract",
          label,
          description: "",
          source: "test",
          destructive: false,
          upstream: "test",
          tool: label,
        },
      ],
    },
  };
}

async function runDebounce() {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(150);
  });
}

async function runImmediate() {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
}

describe("launcher catalog", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    fetchLauncherCatalogMock.mockReset();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("normalizes Labby action and MCP tool entries to stable ids", () => {
    const entries = normalizeLauncherCatalog({
      fingerprint: "fp",
      entries: [
        {
          kind: "mcpTool",
          id: "mcp:github::search",
          contractHash: "contract-github-search",
          label: "search",
          description: "Search repos",
          source: "github",
          destructive: false,
          upstream: "github",
          tool: "search",
        },
        {
          kind: "labbyAction",
          id: "labby:gateway::gateway.list",
          contractHash: "contract-gateway-list",
          label: "gateway: gateway.list",
          description: "List gateway upstreams",
          source: "labby",
          destructive: false,
          service: "gateway",
          action: "gateway.list",
        },
      ],
    });

    expect(entries.map((entry) => entry.id)).toEqual([
      "mcp:github::search",
      "labby:gateway::gateway.list",
    ]);
    expect(entries[0].kind).toBe("mcp_tool");
    expect(entries[1].kind).toBe("labby_action");
    expect(entries.map((entry) => entry.contractHash)).toEqual([
      "contract-github-search",
      "contract-gateway-list",
    ]);
  });

  it("searches name upstream source description and kind", () => {
    const [entry] = normalizeLauncherCatalog({
      fingerprint: "fp",
      entries: [
        {
          kind: "mcpTool",
          id: "mcp:github::search",
          contractHash: "contract-github-search",
          label: "search",
          description: "Search repos",
          source: "github",
          destructive: false,
          upstream: "github",
          tool: "search",
        },
      ],
    });

    expect(launcherEntryMatches(entry, "github")).toBe(true);
    expect(launcherEntryMatches(entry, "repos")).toBe(true);
    expect(launcherEntryMatches(entry, "mcp_tool")).toBe(true);
    expect(launcherEntryMatches(entry, "zzz")).toBe(false);
  });

  it("keeps duplicate visible names distinct by id", () => {
    const entries = normalizeLauncherCatalog({
      fingerprint: "fp",
      entries: [
        {
          kind: "mcpTool",
          id: "mcp:a::search",
          contractHash: "contract-a-search",
          label: "search",
          description: "",
          source: "a",
          destructive: false,
          upstream: "a",
          tool: "search",
        },
        {
          kind: "mcpTool",
          id: "mcp:b::search",
          contractHash: "contract-b-search",
          label: "search",
          description: "",
          source: "b",
          destructive: false,
          upstream: "b",
          tool: "search",
        },
      ],
    });

    expect(new Set(entries.map((entry) => entry.id)).size).toBe(2);
  });

  it("debounces search and ignores a stale completion", async () => {
    let resolveFirst!: (value: ReturnType<typeof catalog>) => void;
    const first = new Promise<ReturnType<typeof catalog>>((resolve) => {
      resolveFirst = resolve;
    });
    fetchLauncherCatalogMock.mockReturnValueOnce(first).mockResolvedValueOnce(catalog("second"));
    const { result, rerender } = renderHook(({ query }) => useLauncherCatalog(query), {
      initialProps: { query: "first" },
    });

    await act(async () => {
      await vi.advanceTimersByTimeAsync(149);
    });
    expect(fetchLauncherCatalogMock).not.toHaveBeenCalled();
    await runDebounce();
    expect(fetchLauncherCatalogMock).toHaveBeenCalledWith("first", null);
    rerender({ query: "second" });
    await runDebounce();
    await act(async () => {
      resolveFirst(catalog("first"));
    });

    expect(result.current.actions.map((entry) => entry.label)).toEqual(["second"]);
  });

  it("reports partial results and clears the state after a complete response", async () => {
    fetchLauncherCatalogMock
      .mockResolvedValueOnce(catalog("partial", true))
      .mockResolvedValueOnce(catalog("complete"));
    const { result, rerender } = renderHook(({ query }) => useLauncherCatalog(query), {
      initialProps: { query: "partial" },
    });

    await runDebounce();
    expect(result.current.truncated).toBe(true);
    expect(result.current.actions).toHaveLength(1);
    rerender({ query: "complete" });
    await runDebounce();

    expect(result.current.truncated).toBe(false);
    expect(result.current.actions[0].label).toBe("complete");
  });

  it("clears a prior partial state when the next request returns an HTTP error", async () => {
    fetchLauncherCatalogMock.mockResolvedValueOnce(catalog("partial", true)).mockResolvedValueOnce({
      ok: false,
      status: 503,
      path: "/v1/palette/search",
      method: "GET",
      payload: { message: "unavailable" },
    });
    const { result, rerender } = renderHook(({ query }) => useLauncherCatalog(query), {
      initialProps: { query: "partial" },
    });

    await runDebounce();
    expect(result.current.truncated).toBe(true);
    rerender({ query: "failure" });
    expect(result.current.truncated).toBe(true);
    await runDebounce();

    expect(result.current.truncated).toBe(false);
    expect(result.current.error).toContain("unavailable");
  });

  it("clears a prior partial state when the next request rejects during decoding", async () => {
    fetchLauncherCatalogMock
      .mockResolvedValueOnce(catalog("partial", true))
      .mockRejectedValueOnce(new Error("invalid shape"));
    const { result, rerender } = renderHook(({ query }) => useLauncherCatalog(query), {
      initialProps: { query: "partial" },
    });

    await runDebounce();
    expect(result.current.truncated).toBe(true);
    rerender({ query: "malformed" });
    expect(result.current.truncated).toBe(true);
    await runDebounce();

    expect(result.current.truncated).toBe(false);
    expect(result.current.error).toBe("invalid shape");
  });

  it("uses the successful unfiltered fingerprint for later conditional requests", async () => {
    fetchLauncherCatalogMock
      .mockResolvedValueOnce(catalog("base"))
      .mockResolvedValueOnce({ notModified: true })
      .mockRejectedValueOnce(new Error("temporary failure"))
      .mockResolvedValueOnce({ notModified: true });
    const { result } = renderHook(() => useLauncherCatalog(""));

    await runImmediate();
    expect(fetchLauncherCatalogMock).toHaveBeenLastCalledWith("", null);
    act(() => result.current.refresh());
    await runImmediate();
    expect(fetchLauncherCatalogMock).toHaveBeenLastCalledWith("", '"base"');
    act(() => result.current.refresh());
    await runImmediate();
    expect(result.current.error).toBe("temporary failure");
    expect(fetchLauncherCatalogMock).toHaveBeenLastCalledWith("", '"base"');
    act(() => result.current.refresh());
    await runImmediate();
    expect(fetchLauncherCatalogMock).toHaveBeenLastCalledWith("", '"base"');
    expect(result.current.error).toBeNull();
  });

  it("preserves an unfiltered partial catalog and warning across a 304", async () => {
    fetchLauncherCatalogMock
      .mockResolvedValueOnce(catalog("partial", true))
      .mockResolvedValueOnce({ notModified: true });
    const { result } = renderHook(() => useLauncherCatalog(""));

    await runImmediate();
    expect(result.current.actions[0].label).toBe("partial");
    expect(result.current.truncated).toBe(true);
    act(() => result.current.refresh());
    await runImmediate();

    expect(fetchLauncherCatalogMock).toHaveBeenLastCalledWith("", '"partial"');
    expect(result.current.actions[0].label).toBe("partial");
    expect(result.current.truncated).toBe(true);
    expect(result.current.error).toBeNull();
  });

  it("surfaces a request error and clears it after recovery", async () => {
    fetchLauncherCatalogMock
      .mockRejectedValueOnce(new Error("catalog unavailable"))
      .mockResolvedValueOnce(catalog("recovered"));
    const { result, rerender } = renderHook(({ query }) => useLauncherCatalog(query), {
      initialProps: { query: "broken" },
    });

    await runDebounce();
    expect(result.current.error).toBe("catalog unavailable");
    rerender({ query: "recovered" });
    await runDebounce();

    expect(result.current.error).toBeNull();
    expect(result.current.actions[0].label).toBe("recovered");
  });
});
