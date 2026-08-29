import { describe, expect, it } from "vitest";

import { endpointStatus, endpointStatusMessage } from "@/lib/endpointStatus";

describe("endpointStatus", () => {
  it("is online after config and catalog load successfully", () => {
    expect(
      endpointStatus({ configured: true, catalogLoading: false, configError: null, catalogError: null }),
    ).toBe("online");
  });

  it("is syncing while config or catalog is loading", () => {
    expect(
      endpointStatus({ configured: false, catalogLoading: false, configError: null, catalogError: null }),
    ).toBe("syncing");
    expect(
      endpointStatus({ configured: true, catalogLoading: true, configError: null, catalogError: null }),
    ).toBe("syncing");
  });

  it("is error when either load fails", () => {
    expect(
      endpointStatus({ configured: true, catalogLoading: false, configError: "bad config", catalogError: null }),
    ).toBe("error");
    expect(
      endpointStatus({ configured: true, catalogLoading: false, configError: null, catalogError: "offline" }),
    ).toBe("error");
  });
});

describe("endpointStatusMessage", () => {
  const input = {
    configured: true,
    catalogLoading: false,
    configError: null,
    catalogError: null,
    endpointLabel: "labby.local",
  };

  it("explains catalog failures and provides recovery", () => {
    expect(endpointStatusMessage({ ...input, catalogError: "401 from catalog" })).toBe(
      "Catalog unavailable: 401 from catalog. Check the server connection in Settings.",
    );
  });

  it("does not recommend an unavailable settings panel when configuration cannot load", () => {
    expect(endpointStatusMessage({ ...input, configured: false, configError: "defaults failed" })).toBe(
      "Configuration unavailable: defaults failed. Restart Labby Palette or inspect the application logs.",
    );
  });

  it("reports loading, syncing, and connected states", () => {
    expect(endpointStatusMessage({ ...input, configured: false })).toBe("Loading server configuration.");
    expect(endpointStatusMessage({ ...input, catalogLoading: true })).toBe(
      "Syncing the catalog from labby.local.",
    );
    expect(endpointStatusMessage(input)).toBe("Connected to labby.local.");
  });
});
