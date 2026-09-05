import type { LauncherEntry } from "@/lib/launcherCatalog";
import type { PaletteResult } from "@/lib/labbyClient";

const STORAGE_KEY = "labby.palette.recentLaunches";
const MAX_RECENT = 50;

export interface PaletteLaunchAudit {
  id: string;
  label: string;
  source: string;
  ok: boolean;
  status: number;
  at: string;
  receipt?: PaletteReceiptAudit;
}

export interface PaletteReceiptAudit {
  requestId: string;
  toolId: string;
  contractHash: string;
  catalogRevision: string;
  executionMode?: "exact" | "labby_action";
  truncated: boolean;
}

export function recordPaletteLaunch(action: LauncherEntry, result: PaletteResult): void {
  try {
    if (typeof window === "undefined" || !window.localStorage) return;
    const current = readPaletteLaunches();
    const entry: PaletteLaunchAudit = {
      id: action.id,
      label: action.label,
      source: action.source,
      ok: result.ok,
      status: result.status,
      at: new Date().toISOString(),
      receipt: paletteReceipt(result.payload),
    };
    window.localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify([entry, ...current].slice(0, MAX_RECENT)),
    );
  } catch {
    // Audit history is useful operator context, but it must never affect execution.
  }
}

function paletteReceipt(payload: unknown): PaletteReceiptAudit | undefined {
  if (!payload || typeof payload !== "object" || Array.isArray(payload)) return undefined;
  const receipt = (payload as Record<string, unknown>).receipt;
  if (!receipt || typeof receipt !== "object" || Array.isArray(receipt)) return undefined;
  const value = receipt as Record<string, unknown>;
  if (
    typeof value.requestId !== "string" ||
    typeof value.toolId !== "string" ||
    typeof value.contractHash !== "string" ||
    typeof value.catalogRevision !== "string" ||
    typeof value.truncated !== "boolean"
  )
    return undefined;
  return {
    requestId: value.requestId,
    toolId: value.toolId,
    contractHash: value.contractHash,
    catalogRevision: value.catalogRevision,
    ...(value.executionMode === "exact" || value.executionMode === "labby_action"
      ? { executionMode: value.executionMode }
      : {}),
    truncated: value.truncated,
  };
}

export function readPaletteLaunches(): PaletteLaunchAudit[] {
  try {
    if (typeof window === "undefined" || !window.localStorage) return [];
    const parsed = JSON.parse(window.localStorage.getItem(STORAGE_KEY) ?? "[]");
    return Array.isArray(parsed)
      ? parsed.filter(isPaletteLaunchAudit).map((entry) => ({
          ...entry,
          receipt: paletteReceipt(entry),
        }))
      : [];
  } catch {
    return [];
  }
}

function isPaletteLaunchAudit(value: unknown): value is PaletteLaunchAudit {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const record = value as Record<string, unknown>;
  return (
    typeof record.id === "string" &&
    typeof record.label === "string" &&
    typeof record.source === "string" &&
    typeof record.ok === "boolean" &&
    typeof record.status === "number" &&
    typeof record.at === "string"
  );
}
