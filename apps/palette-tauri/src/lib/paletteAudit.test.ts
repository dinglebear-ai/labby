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

  it("records recent launches with redacted params", () => {
    recordPaletteLaunch(
      action,
      { query: "labby", token: "secret-token" },
      {
        ok: true,
        status: 200,
        path: "/v1/palette/execute",
        method: "POST",
        payload: { ok: true },
      },
    );

    expect(readPaletteLaunches()).toMatchObject([
      {
        id: "mcp:github::search_repos",
        label: "search_repos",
        source: "github",
        ok: true,
        status: 200,
        params: { query: "labby", token: "[REDACTED]" },
      },
    ]);
  });

  it("ignores localStorage write failures", () => {
    const setItem = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("quota exceeded");
    });

    expect(() =>
      recordPaletteLaunch(
        action,
        { query: "labby" },
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
