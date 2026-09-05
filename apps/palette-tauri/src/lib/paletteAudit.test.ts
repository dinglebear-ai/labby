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
    recordPaletteLaunch(action, {
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
    });

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
      recordPaletteLaunch(action, {
        ok: true,
        status: 200,
        path: "/v1/palette/execute",
        method: "POST",
        payload: { ok: true },
      }),
    ).not.toThrow();

    setItem.mockRestore();
  });

  it.each([
    "exact",
    "labby_action",
  ])("preserves %s execution evidence across storage reads", (executionMode) => {
    recordPaletteLaunch(action, {
      ok: true,
      status: 200,
      path: "/v1/palette/execute",
      method: "POST",
      payload: {
        receipt: {
          requestId: "req-mode",
          toolId: action.id,
          contractHash: action.contractHash,
          catalogRevision: "pool:42",
          executionMode,
          truncated: false,
        },
      },
    });
    expect(readPaletteLaunches()[0].receipt?.executionMode).toBe(executionMode);
    expect(
      JSON.parse(window.localStorage.getItem("labby.palette.recentLaunches") ?? "[]")[0].receipt
        .executionMode,
    ).toBe(executionMode);
  });

  it.each([
    undefined,
    "delegated",
    0,
    null,
    { secret: "MODE-CANARY" },
  ])("does not invent execution evidence for legacy or invalid mode %j", (executionMode) => {
    const receipt = {
      requestId: "req-legacy",
      toolId: action.id,
      contractHash: action.contractHash,
      catalogRevision: "pool:42",
      truncated: false,
      executionMode,
    };
    recordPaletteLaunch(action, {
      ok: true,
      status: 200,
      path: "/v1/palette/execute",
      method: "POST",
      payload: { receipt },
    });
    expect(readPaletteLaunches()[0].receipt).toMatchObject({ requestId: "req-legacy" });
    expect(readPaletteLaunches()[0].receipt).not.toHaveProperty("executionMode");
    // Revalidate old or externally modified history as well as new responses.
    const stored = readPaletteLaunches()[0];
    window.localStorage.setItem(
      "labby.palette.recentLaunches",
      JSON.stringify([{ ...stored, receipt }]),
    );
    expect(readPaletteLaunches()[0].receipt).not.toHaveProperty("executionMode");
  });
});
