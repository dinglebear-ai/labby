import { describe, expect, it } from "vitest";

import { endpointStatus } from "@/lib/endpointStatus";

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
