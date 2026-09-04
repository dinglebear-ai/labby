import { describe, expect, it } from "vitest";

import { endpointStatus, endpointStatusMessage } from "@/lib/endpointStatus";

describe("endpointStatus", () => {
  it("is online after config and catalog load successfully", () => {
    expect(
      endpointStatus({
        configured: true,
        catalogLoading: false,
        configError: null,
        catalogError: null,
      }),
    ).toBe("online");
  });

  it("is syncing while config or catalog is loading", () => {
    expect(
      endpointStatus({
        configured: false,
        catalogLoading: false,
        configError: null,
        catalogError: null,
      }),
    ).toBe("syncing");
    expect(
      endpointStatus({
        configured: true,
        catalogLoading: true,
        configError: null,
        catalogError: null,
      }),
    ).toBe("syncing");
  });

  it("is error when either load fails", () => {
    expect(
      endpointStatus({
        configured: true,
        catalogLoading: false,
        configError: "bad config",
        catalogError: null,
      }),
    ).toBe("error");
    expect(
      endpointStatus({
        configured: true,
        catalogLoading: false,
        configError: null,
        catalogError: "offline",
      }),
    ).toBe("error");
  });

  it("keeps errors visible while the catalog is also loading", () => {
    expect(
      endpointStatus({
        configured: true,
        catalogLoading: true,
        configError: "bad config",
        catalogError: null,
      }),
    ).toBe("error");
    expect(
      endpointStatus({
        configured: true,
        catalogLoading: true,
        configError: null,
        catalogError: "offline",
      }),
    ).toBe("error");
  });
});

describe("endpointStatusMessage", () => {
  const ready = {
    configured: true,
    catalogLoading: false,
    configError: null,
    catalogError: null,
    endpointLabel: "localhost",
  };
  it("describes loading and ready states", () => {
    expect(endpointStatusMessage(ready)).toBe("Connected to localhost.");
    expect(endpointStatusMessage({ ...ready, configured: false })).toBe(
      "Loading server configuration.",
    );
    expect(endpointStatusMessage({ ...ready, catalogLoading: true })).toBe(
      "Syncing the catalog from localhost.",
    );
  });
  it("prioritizes actionable errors without repeating potentially sensitive diagnostics", () => {
    expect(
      endpointStatusMessage({
        ...ready,
        configError: "secret",
        catalogError: "secret",
        catalogLoading: true,
      }),
    ).toBe("Configuration unavailable. Restart Labby Palette and try again.");
    expect(endpointStatusMessage({ ...ready, catalogError: "secret", catalogLoading: true })).toBe(
      "Catalog unavailable. Check the server connection in Settings.",
    );
  });
});
