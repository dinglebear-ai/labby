import "@testing-library/jest-dom/vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { GatewayWorkspace } from "./GatewayWorkspace";
import { gatewayClient } from "@/lib/labby/gateway/client";
import { fetchCatalog } from "@/lib/labbyClient";
import { useOauthSession } from "@/lib/useOauthSession";

vi.mock("@/lib/labby/gateway/client", () => ({
  gatewayClient: {
    list: vi.fn(),
    get: vi.fn(),
    testDraft: vi.fn(),
    create: vi.fn(),
    update: vi.fn(),
    remove: vi.fn(),
    reload: vi.fn(),
  },
  GatewayClientError: class extends Error {},
}));
vi.mock("@/lib/labbyClient", () => ({ fetchCatalog: vi.fn() }));
vi.mock("@/lib/useOauthSession", () => ({ useOauthSession: vi.fn() }));

const config = {
  serverUrl: "https://labby.test",
  staticToken: null,
  shortcut: "Ctrl+Shift+Space",
  theme: "dark" as const,
  hideOnBlur: false,
};
const adminSession = {
  status: {
    signedIn: true,
    scope: "lab:admin",
    expiresAtUnix: null,
    serverUrl: "https://labby.test",
  },
  busy: false,
  error: null,
  view: { label: "Signed in", detail: "", tone: "success" },
  signIn: vi.fn(),
  signOut: vi.fn(),
};

beforeEach(() => {
  vi.mocked(fetchCatalog).mockResolvedValue({
    notModified: false,
    catalog: {
      services: [
        {
          name: "gateway",
          description: "",
          category: "",
          status: "available",
          actions: [
            { name: "gateway.list", description: "", destructive: false, params: [], returns: "" },
            { name: "gateway.add", description: "", destructive: false, params: [], returns: "" },
            {
              name: "gateway.update",
              description: "",
              destructive: false,
              params: [],
              returns: "",
            },
          ],
        },
      ],
    },
  });
  vi.mocked(useOauthSession).mockReturnValue(adminSession as ReturnType<typeof useOauthSession>);
  vi.mocked(gatewayClient.list).mockResolvedValue([]);
});

describe("GatewayWorkspace", () => {
  it("fails closed when admin authorization is absent", async () => {
    vi.mocked(useOauthSession).mockReturnValue({
      ...adminSession,
      status: { ...adminSession.status, scope: "lab:read" },
    } as ReturnType<typeof useOauthSession>);
    render(<GatewayWorkspace config={config} onClose={vi.fn()} />);
    expect(await screen.findByText(/administrator authorization required/i)).toBeInTheDocument();
  });

  it("shows unavailable capability state", async () => {
    vi.mocked(fetchCatalog).mockResolvedValue({ notModified: false, catalog: { services: [] } });
    render(<GatewayWorkspace config={config} onClose={vi.fn()} />);
    expect(await screen.findByText(/gateway management unavailable/i)).toBeInTheDocument();
  });

  it("requires confirmation and shows stdio disclosure before testing", async () => {
    render(<GatewayWorkspace config={config} onClose={vi.fn()} />);
    await screen.findByText(/0 upstreams/i);
    fireEvent.click(screen.getByRole("button", { name: /add upstream/i }));
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "local" } });
    fireEvent.change(screen.getByLabelText("Transport"), { target: { value: "stdio" } });
    fireEvent.change(screen.getByLabelText("Command"), { target: { value: "npx" } });
    expect(screen.getByText(/may start the configured command/i)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Test" }));
    expect(screen.getByRole("alertdialog")).toHaveTextContent(/confirm test/i);
    expect(gatewayClient.testDraft).not.toHaveBeenCalled();
  });

  it("renders a bounded unavailable-runtime error", async () => {
    vi.mocked(gatewayClient.list).mockRejectedValue(new Error("offline"));
    render(<GatewayWorkspace config={config} onClose={vi.fn()} />);
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("offline"));
  });
});
