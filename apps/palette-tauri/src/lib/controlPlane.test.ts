import { describe, expect, it, vi } from "vitest";

import { controlPlaneErrorMessage, controlPlanePath, launchControlPlane } from "@/lib/controlPlane";
import { invoke } from "@/lib/invoke";
import type { LauncherEntry } from "@/lib/launcherCatalog";

vi.mock("@/lib/invoke", () => ({ invoke: vi.fn() }));

function entry(id: string): LauncherEntry {
  return { id } as LauncherEntry;
}

describe("controlPlanePath", () => {
  it("deep-links managed artifacts and gateway actions", () => {
    expect(controlPlanePath(entry("labby:artifacts::artifacts.list"))).toBe(
      "/skills/?view=control-plane",
    );
    expect(controlPlanePath(entry("labby:jobs::jobs.list"))).toBe("/skills/?view=control-plane");
    expect(controlPlanePath(entry("labby:uploads::uploads.list"))).toBe(
      "/skills/?view=control-plane",
    );
    expect(controlPlanePath(entry("labby:gateway::gateway.list"))).toBe("/gateways/");
  });

  it("opens upstream tools in the tool browser and unknown services at home", () => {
    expect(controlPlanePath(entry("mcp:github::search"))).toBe("/tools/");
    expect(controlPlanePath(entry("labby:future::future.list"))).toBe("/");
  });
});

describe("launchControlPlane", () => {
  it("surfaces invoke failures without leaving an unhandled rejection", async () => {
    vi.mocked(invoke).mockRejectedValueOnce(new Error("settings unavailable"));
    const errors: string[] = [];

    await launchControlPlane("/skills/", (message) => errors.push(message));

    expect(invoke).toHaveBeenCalledWith("open_control_plane", {
      path: "/skills/",
    });
    expect(errors).toEqual(["Could not open the Control Plane: settings unavailable"]);
  });

  it("normalizes non-Error rejection values", () => {
    expect(controlPlaneErrorMessage("offline")).toBe("Could not open the Control Plane: offline");
  });

  it("does not surface an older failure after a newer launch succeeds", async () => {
    let rejectOlder!: (error: Error) => void;
    const older = new Promise<void>((_, reject) => {
      rejectOlder = reject;
    });
    vi.mocked(invoke).mockReturnValueOnce(older).mockResolvedValueOnce(undefined);
    const errors: string[] = [];

    const first = launchControlPlane("/skills/", (message) => errors.push(message));
    await launchControlPlane("/gateways/", (message) => errors.push(message));
    rejectOlder(new Error("late failure"));
    await first;

    expect(errors).toEqual([]);
  });
});
