// @vitest-environment node
import { describe, expect, it } from "vitest";

import { resolvePaletteWindowSize } from "./useWindowChrome";

const SCREEN = { width: 2560, height: 1440 };

describe("resolvePaletteWindowSize", () => {
  it("uses the settings window when settings are open", () => {
    expect(
      resolvePaletteWindowSize(
        { settingsOpen: true, showResultsLayout: false, showContent: true },
        SCREEN,
        () => 468,
      ),
    ).toEqual({ width: 800, height: 560 });
  });

  it("uses a roomy result window (capped by the screen margin) for results", () => {
    expect(
      resolvePaletteWindowSize(
        { settingsOpen: false, showResultsLayout: true, showContent: true },
        SCREEN,
        () => 468,
      ),
    ).toEqual({ width: 1280, height: 860 });
  });

  it("hugs the measured browse height while content is shown", () => {
    expect(
      resolvePaletteWindowSize(
        { settingsOpen: false, showResultsLayout: false, showContent: true },
        SCREEN,
        () => 468,
      ),
    ).toEqual({ width: 760, height: 468 });
  });

  it("falls back to the compact launcher when nothing is shown", () => {
    expect(
      resolvePaletteWindowSize(
        { settingsOpen: false, showResultsLayout: false, showContent: false },
        SCREEN,
        () => 468,
      ),
    ).toEqual({ width: 720, height: 92 });
  });
});
