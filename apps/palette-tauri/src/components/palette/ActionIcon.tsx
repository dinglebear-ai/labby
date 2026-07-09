import {
  Activity,
  Bell,
  Bot,
  Boxes,
  Download,
  FileText,
  Film,
  Network,
  PlugZap,
  Search,
  StickyNote,
  Terminal,
  type LucideIcon,
} from "lucide-react";

import type { LauncherEntry } from "@/lib/launcherCatalog";

// Category → icon. Labby's catalog categories are one of a fixed set (see
// PluginMeta::Category); anything unknown falls back to a generic terminal glyph.
const CATEGORY_ICONS: Record<string, LucideIcon> = {
  media: Film,
  servarr: Boxes,
  indexer: Search,
  download: Download,
  notes: StickyNote,
  documents: FileText,
  network: Network,
  notifications: Bell,
  ai: Bot,
  bootstrap: Activity,
};

/** Action-list / command-bar icon for an action, derived from its category. */
export function actionIcon(category: string): LucideIcon {
  return CATEGORY_ICONS[category.toLowerCase()] ?? Terminal;
}

export function ActionIcon({ action, selected }: { action: LauncherEntry; selected: boolean }) {
  const Icon = action.kind === "mcp_tool" ? PlugZap : actionIcon(action.category);
  return (
    <span
      className={`action-icon${selected ? " action-icon-selected" : ""}`}
      aria-hidden="true"
    >
      <Icon size={16} strokeWidth={1.65} />
    </span>
  );
}
