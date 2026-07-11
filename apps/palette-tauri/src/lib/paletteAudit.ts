import type { LauncherEntry } from "@/lib/launcherCatalog";
import type { PaletteResult } from "@/lib/labbyClient";
import { redactLauncherParams } from "@/lib/launcherValidation";

const STORAGE_KEY = "labby.palette.recentLaunches";
const MAX_RECENT = 50;

export interface PaletteLaunchAudit {
  id: string;
  label: string;
  source: string;
  ok: boolean;
  status: number;
  at: string;
  params: unknown;
}

export function recordPaletteLaunch(action: LauncherEntry, params: unknown, result: PaletteResult): void {
  if (typeof window === "undefined" || !window.localStorage) return;
  const current = readPaletteLaunches();
  const entry: PaletteLaunchAudit = {
    id: action.id,
    label: action.label,
    source: action.source,
    ok: result.ok,
    status: result.status,
    at: new Date().toISOString(),
    params: redactLauncherParams(params),
  };
  window.localStorage.setItem(STORAGE_KEY, JSON.stringify([entry, ...current].slice(0, MAX_RECENT)));
}

export function readPaletteLaunches(): PaletteLaunchAudit[] {
  if (typeof window === "undefined" || !window.localStorage) return [];
  try {
    const parsed = JSON.parse(window.localStorage.getItem(STORAGE_KEY) ?? "[]");
    return Array.isArray(parsed) ? parsed.filter(isPaletteLaunchAudit) : [];
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
