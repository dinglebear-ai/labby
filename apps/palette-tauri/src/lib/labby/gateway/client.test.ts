import { beforeEach, describe, expect, it, vi } from "vitest";
import { dispatchAction } from "@/lib/labbyClient";
import { gatewayClient, StaleGatewayError } from "./client";
import { emptyGatewayDraft, type GatewayView, gatewayFingerprint } from "./model";

vi.mock("@/lib/labbyClient", async (original) => ({
  ...(await original()),
  dispatchAction: vi.fn(),
}));
const dispatch = vi.mocked(dispatchAction);
const view: GatewayView = {
  revision: "sha256:fixture",
  config: {
    name: "docs",
    enabled: true,
    url: "https://example.test/mcp",
    args: [],
    oauth_enabled: false,
    proxy_resources: false,
    proxy_prompts: false,
  },
  runtime: { name: "docs", connected: true, tool_count: 1, resource_count: 0, prompt_count: 0 },
};

beforeEach(() => dispatch.mockReset());

describe("gateway client", () => {
  it("routes add and draft test through Labby-owned gateway actions", async () => {
    dispatch.mockResolvedValue({
      ok: true,
      status: 200,
      path: "/v1/gateway",
      method: "POST",
      payload: view,
    });
    const draft = { ...emptyGatewayDraft(), name: "docs", url: "https://example.test/mcp" };
    await gatewayClient.testDraft(draft);
    await gatewayClient.create(draft);
    expect(dispatch.mock.calls.map((call) => call[1])).toEqual(["gateway.test", "gateway.add"]);
  });

  it("hydrates custom gateway rows through the typed detail action", async () => {
    dispatch
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        path: "/v1/gateway",
        method: "POST",
        payload: [
          { id: "docs", source: "custom_gateway" },
          { id: "lab", source: "in_process" },
        ],
      })
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        path: "/v1/gateway",
        method: "POST",
        payload: view,
      });
    await expect(gatewayClient.list()).resolves.toEqual([view]);
    expect(dispatch.mock.calls.map((call) => call[1])).toEqual(["gateway.list", "gateway.get"]);
  });

  it("fails closed before update when the server snapshot is stale", async () => {
    dispatch.mockResolvedValue({
      ok: true,
      status: 200,
      path: "/v1/gateway",
      method: "POST",
      payload: {
        ...view,
        revision: "sha256:changed",
        config: { ...view.config, enabled: false },
      },
    });
    await expect(
      gatewayClient.update("docs", emptyGatewayDraft(), gatewayFingerprint(view)),
    ).rejects.toBeInstanceOf(StaleGatewayError);
    expect(dispatch).toHaveBeenCalledTimes(1);
  });

  it("routes revision-checked update, remove, and reload operations", async () => {
    dispatch.mockResolvedValue({
      ok: true,
      status: 200,
      path: "/v1/gateway",
      method: "POST",
      payload: view,
    });
    const expected = gatewayFingerprint(view);
    await gatewayClient.update("docs", { ...emptyGatewayDraft(), name: "docs" }, expected);
    await gatewayClient.remove("docs", expected);
    await gatewayClient.reload("docs", expected);
    expect(dispatch.mock.calls.map((call) => call[1])).toEqual([
      "gateway.get",
      "gateway.update",
      "gateway.get",
      "gateway.remove",
      "gateway.get",
      "gateway.reload",
      "gateway.get",
    ]);
  });

  it("surfaces redirects, auth challenges, and unavailable runtime errors", async () => {
    for (const [status, kind] of [
      [302, "redirect_rejected"],
      [401, "unauthorized"],
      [503, "runtime_unavailable"],
    ] as const) {
      dispatch.mockResolvedValueOnce({
        ok: false,
        status,
        path: "/v1/gateway",
        method: "POST",
        payload: { kind, message: kind },
      });
      await expect(gatewayClient.list()).rejects.toMatchObject({ kind, status });
    }
  });
});
