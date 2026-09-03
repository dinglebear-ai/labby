import { describe, expect, it } from "vitest";

import { controlPlanePath } from "@/lib/controlPlane";
import type { LauncherEntry } from "@/lib/launcherCatalog";

function entry(id: string): LauncherEntry {
  return { id } as LauncherEntry;
}

describe("controlPlanePath", () => {
  it("deep-links managed artifacts and gateway actions", () => {
    expect(controlPlanePath(entry("labby:artifacts::artifacts.list"))).toBe("/skills/");
    expect(controlPlanePath(entry("labby:gateway::gateway.list"))).toBe("/gateways/");
  });

  it("opens upstream tools in the tool browser and unknown services at home", () => {
    expect(controlPlanePath(entry("mcp:github::search"))).toBe("/tools/");
    expect(controlPlanePath(entry("labby:future::future.list"))).toBe("/");
  });
});
