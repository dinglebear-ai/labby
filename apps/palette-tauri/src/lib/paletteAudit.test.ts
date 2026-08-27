import { beforeEach, describe, expect, it, vi } from "vitest";

import type { LauncherEntry } from "@/lib/launcherCatalog";
import { readPaletteLaunches, recordPaletteLaunch } from "@/lib/paletteAudit";

const action: LauncherEntry = {
  kind: "mcp_tool",
  id: "mcp:github::search_repos",
  subcommand: "mcp:github::search_repos",
  service: "github",
  action: "search_repos",
  label: "search_repos",
  description: "",
  category: "mcp",
  source: "github",
  destructive: false,
  contractHash: "a".repeat(64),
  params: [],
  argMode: "json",
  schemaFingerprint: "fp",
  upstream: "github",
  tool: "search_repos",
  searchText: "",
};

describe("palette audit trail", () => {
  beforeEach(() => {
    window.localStorage.clear();
  });

  it("records receipt metadata without persisting any parameter values", () => {
    const canary = "INNOCUOUS-PARAM-CANARY";
    recordPaletteLaunch(
      action,
      {
        ok: true,
        status: 200,
        path: "/v1/palette/execute",
        method: "POST",
        payload: {
          harmless: canary,
          receipt: {
            requestId: "req-123",
            toolId: action.id,
            contractHash: action.contractHash,
            catalogRevision: "pool:42",
            truncated: false,
          },
        },
      },
    );

    expect(readPaletteLaunches()).toMatchObject([
      {
        id: "mcp:github::search_repos",
        label: "search_repos",
        source: "github",
        ok: true,
        status: 200,
        receipt: {
          requestId: "req-123",
          toolId: action.id,
          contractHash: action.contractHash,
          catalogRevision: "pool:42",
          truncated: false,
        },
      },
    ]);
    const persisted = window.localStorage.getItem("labby.palette.recentLaunches") ?? "";
    expect(persisted).not.toContain(canary);
    expect(persisted).not.toContain("params");
  });

  it("ignores localStorage write failures", () => {
    const setItem = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("quota exceeded");
    });

    expect(() =>
      recordPaletteLaunch(
        action,
        {
          ok: true,
          status: 200,
          path: "/v1/palette/execute",
          method: "POST",
          payload: { ok: true },
        },
      ),
    ).not.toThrow();

    setItem.mockRestore();
  });
});
