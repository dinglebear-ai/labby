// Small view helpers shared by the command bar, action list, and lifecycle
// hooks. Generic (catalog-driven) — no per-action Axon knowledge.

import type { PaletteAction } from "@/lib/actions";
import type { LauncherEntry } from "@/lib/launcherCatalog";

export const COMMAND_INPUT_SELECTOR = ".command-input";

/** Focus the command input, optionally selecting its contents. */
export function focusInput(select = false): void {
  // Deferred a tick so it runs after any pending re-render swaps the input in.
  window.setTimeout(() => {
    const input = document.querySelector<HTMLInputElement>(COMMAND_INPUT_SELECTOR);
    if (!input) return;
    input.focus();
    if (select) input.select();
  }, 0);
}

/** Rank matched actions by relevance to `query` (exact/prefix first). */
export function sortActionsByRelevance(actions: PaletteAction[], query: string): PaletteAction[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return sortActionsForDisplay(actions);
  return [...actions].sort((a, b) => relevance(b, needle) - relevance(a, needle));
}

function relevance(action: PaletteAction, needle: string): number {
  const sub = action.subcommand.toLowerCase();
  if (sub === needle) return 100;
  if (sub.startsWith(needle)) return 60;
  if (action.action.toLowerCase().startsWith(needle)) return 50;
  if (sub.includes(needle)) return 30;
  if (action.label.toLowerCase().includes(needle)) return 20;
  return 0;
}

/** Stable display order: grouped by category, then by subcommand. */
export function sortActionsForDisplay(actions: PaletteAction[]): PaletteAction[] {
  return [...actions].sort((a, b) => {
    const byCategory = a.category.localeCompare(b.category);
    return byCategory !== 0 ? byCategory : a.subcommand.localeCompare(b.subcommand);
  });
}

/** Placeholder text for the argument input of an action in mode. */
export function argumentPlaceholder(action: PaletteAction | LauncherEntry): string {
  if (action.argMode === "none") return "Press Enter to run";
  if (action.argMode === "json") {
    const required = action.params.filter((param) => param.required).map((param) => param.name);
    return required.length > 0
      ? `JSON params — e.g. {"${required[0]}": …}`
      : `JSON params (optional)`;
  }
  const first = action.params[0];
  return first ? `${first.name}${first.required ? "" : " (optional)"}` : "Argument";
}
