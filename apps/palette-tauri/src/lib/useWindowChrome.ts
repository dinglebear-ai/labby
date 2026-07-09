import { useEffect, useRef } from "react";

import { invoke } from "@/lib/invoke";

interface WindowChromeArgs {
  settingsOpen: boolean;
  showResultsLayout: boolean;
  showContent: boolean;
  filteredLength: number;
  shownTick: number;
}

interface PaletteScreen {
  width: number;
  height: number;
}

type BrowseHeight = () => number;

// The palette is a borderless window resized to hug each view; these are the
// per-view logical-px dimensions. `resize_palette` sizes in logical px, so
// CSS-px measurements map 1:1 across DPIs. Geometry must match
// show_main_window() in src-tauri/src/lib.rs — COMPACT = 720×92 bar.
const COMPACT = { width: 720, height: 92 };
const SETTINGS = { width: 800, height: 560 };
const BROWSE_WIDTH = 760;
const RESULTS_MAX = { width: 1280, height: 860 };
const SCREEN_MARGIN = 120;
const BROWSE_SCREEN_MARGIN = 80;

const ACTION_SCROLL_VIEWPORT_SELECTOR = ".action-scroll-viewport";
const FALLBACK_ROW_HEIGHT = 48;
const BROWSE_CHROME = 142;
const LIST_CAP = 338;

export function resolvePaletteWindowSize(
  { settingsOpen, showResultsLayout, showContent }: Omit<WindowChromeArgs, "shownTick" | "filteredLength">,
  screen: PaletteScreen,
  browseHeight: BrowseHeight,
): { width: number; height: number } {
  if (settingsOpen) return SETTINGS;
  if (showResultsLayout) {
    return {
      width: Math.min(RESULTS_MAX.width, screen.width - SCREEN_MARGIN),
      height: Math.min(RESULTS_MAX.height, screen.height - SCREEN_MARGIN),
    };
  }
  if (showContent) {
    return {
      width: BROWSE_WIDTH,
      height: Math.min(browseHeight(), screen.height - BROWSE_SCREEN_MARGIN),
    };
  }
  return COMPACT;
}

// Owns the native window's size/visibility behavior for the palette: it resizes
// the borderless window to fit the current view, and suppresses hide-on-blur
// while a result/settings view is open so the window doesn't vanish when the
// user drags to resize it or clicks another window to review a response.
export function useWindowChrome({
  settingsOpen,
  showResultsLayout,
  showContent,
  filteredLength,
  shownTick,
}: WindowChromeArgs) {
  const lastSizeRef = useRef<{ width: number; height: number } | null>(null);
  const lastShownTickRef = useRef(shownTick);

  useEffect(() => {
    const browseHeight = () => {
      const viewport = document.querySelector(ACTION_SCROLL_VIEWPORT_SELECTOR);
      if (!(viewport instanceof HTMLElement)) {
        return BROWSE_CHROME + filteredLength * FALLBACK_ROW_HEIGHT;
      }
      return BROWSE_CHROME + Math.min(viewport.scrollHeight, LIST_CAP);
    };
    const size = resolvePaletteWindowSize(
      { settingsOpen, showResultsLayout, showContent },
      { width: window.screen.availWidth, height: window.screen.availHeight },
      browseHeight,
    );
    const justShown = lastShownTickRef.current !== shownTick;
    lastShownTickRef.current = shownTick;
    if (
      !justShown &&
      lastSizeRef.current?.width === size.width &&
      lastSizeRef.current?.height === size.height
    ) {
      return;
    }
    lastSizeRef.current = size;
    const floating = size === COMPACT;
    void invoke("resize_palette", { ...size, shadow: !floating }).catch((error) => {
      console.warn("Failed to resize palette window", error);
    });
  }, [settingsOpen, showResultsLayout, showContent, filteredLength, shownTick]);

  useEffect(() => {
    void invoke("set_blur_dismiss", { enabled: !showResultsLayout });
  }, [showResultsLayout]);
}
