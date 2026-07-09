import { describe, expect, it } from "vitest";
import { launcherEntryMatches, normalizeLauncherCatalog } from "./launcherCatalog";

describe("launcher catalog", () => {
  it("normalizes Labby action and MCP tool entries to stable ids", () => {
    const entries = normalizeLauncherCatalog({
      fingerprint: "fp",
      entries: [
        {
          kind: "mcpTool",
          id: "mcp:github::search",
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
          label: "gateway: gateway.list",
          description: "List gateway upstreams",
          source: "labby",
          destructive: false,
          service: "gateway",
          action: "gateway.list",
        },
      ],
    });

    expect(entries.map((entry) => entry.id)).toEqual(["labby:gateway::gateway.list", "mcp:github::search"]);
    expect(entries[0].kind).toBe("labby_action");
    expect(entries[1].kind).toBe("mcp_tool");
  });

  it("searches name upstream source description and kind", () => {
    const [entry] = normalizeLauncherCatalog({
      fingerprint: "fp",
      entries: [
        {
          kind: "mcpTool",
          id: "mcp:github::search",
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
});
