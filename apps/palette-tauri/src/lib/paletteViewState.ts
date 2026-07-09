// Minimal palette view-state reducer: which overlay is open (settings) and
// which action, if any, is in argument-entry mode. Job/crawl/history/browse
// views from the Axon palette are dropped in v1.

import type { PaletteAction } from "@/lib/actions";

export interface PaletteView {
  /** The action currently in argument-entry mode, or null in browse mode. */
  mode: PaletteAction | null;
  settingsOpen: boolean;
}

export const INITIAL_VIEW: PaletteView = { mode: null, settingsOpen: false };

export type ViewIntent =
  | { type: "enterMode"; action: PaletteAction }
  | { type: "clearMode" }
  | { type: "openSettings" }
  | { type: "closeSettings" }
  | { type: "toggleSettings" }
  | { type: "reset" };

export function viewReducer(state: PaletteView, intent: ViewIntent): PaletteView {
  switch (intent.type) {
    case "enterMode":
      return { mode: intent.action, settingsOpen: false };
    case "clearMode":
      return { ...state, mode: null };
    case "openSettings":
      return { ...state, settingsOpen: true };
    case "closeSettings":
      return { ...state, settingsOpen: false };
    case "toggleSettings":
      return { ...state, settingsOpen: !state.settingsOpen };
    case "reset":
      return INITIAL_VIEW;
    default:
      return state;
  }
}

export function modeOf(view: PaletteView): PaletteAction | null {
  return view.mode;
}

export function isSettingsOpen(view: PaletteView): boolean {
  return view.settingsOpen;
}
