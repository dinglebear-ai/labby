import { describe, expect, it } from "vitest";
import { invoke } from "./invoke";

describe("browser invoke launcher fallbacks", () => {
  it("fetch_launcher_catalog returns an empty catalog", async () => {
    await expect(invoke("fetch_launcher_catalog")).resolves.toEqual({
      ok: true,
      status: 200,
      payload: { fingerprint: "browser-fallback", entries: [] },
    });
  });

  it("execute_launcher_entry returns unsupported_surface", async () => {
    await expect(invoke("execute_launcher_entry")).resolves.toEqual({
      ok: false,
      status: 501,
      payload: {
        kind: "unsupported_surface",
        message: "Launcher execution is only available in the desktop app",
      },
    });
  });
});
