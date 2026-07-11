import { useCallback, useEffect, useRef, useState } from "react";

import type { ArgMode, ParamEntry } from "@/lib/actions";
import { type LauncherCatalog, type LauncherEntry as BridgeLauncherEntry, fetchLauncherCatalog } from "@/lib/labbyClient";
import { resultErrorMessage } from "@/lib/labbyClient";

export type LauncherEntry = NormalizedLabbyActionEntry | NormalizedMcpToolEntry;

interface BaseLauncherEntry {
  kind: "labby_action" | "mcp_tool";
  id: string;
  subcommand: string;
  service: string;
  action: string;
  label: string;
  description: string;
  category: string;
  source: string;
  destructive: boolean;
  params: ParamEntry[];
  argMode: ArgMode;
  inputSchema?: unknown;
  schemaFingerprint?: string | null;
  searchText: string;
}

export interface NormalizedLabbyActionEntry extends BaseLauncherEntry {
  kind: "labby_action";
}

export interface NormalizedMcpToolEntry extends BaseLauncherEntry {
  kind: "mcp_tool";
  upstream: string;
  tool: string;
}

export function normalizeLauncherCatalog(catalog: LauncherCatalog): LauncherEntry[] {
  return (catalog.entries ?? []).map(normalizeEntry);
}

function normalizeEntry(entry: BridgeLauncherEntry): LauncherEntry {
  if (entry.kind === "mcpTool") {
    const normalized: NormalizedMcpToolEntry = {
      kind: "mcp_tool",
      id: entry.id,
      subcommand: entry.id,
      service: entry.upstream,
      action: entry.tool,
      label: entry.label || `${entry.upstream}: ${entry.tool}`,
      description: entry.description || "",
      category: "mcp",
      source: entry.source || entry.upstream,
      destructive: Boolean(entry.destructive),
      params: [],
      argMode: "json",
      inputSchema: entry.inputSchema,
      schemaFingerprint: entry.schemaFingerprint ?? null,
      upstream: entry.upstream,
      tool: entry.tool,
      searchText: "",
    };
    normalized.searchText = searchTextFor(normalized);
    return normalized;
  }
  const normalized: NormalizedLabbyActionEntry = {
    kind: "labby_action",
    id: entry.id,
    subcommand: entry.id,
    service: entry.service,
    action: entry.action,
    label: entry.label || `${entry.service}: ${entry.action}`,
    description: entry.description || "",
    category: "labby",
    source: entry.source || entry.service,
    destructive: Boolean(entry.destructive),
    params: [],
    argMode: entry.inputSchema || entry.schemaFingerprint ? "json" : "none",
    inputSchema: entry.inputSchema,
    schemaFingerprint: entry.schemaFingerprint ?? null,
    searchText: "",
  };
  normalized.searchText = searchTextFor(normalized);
  return normalized;
}

export function launcherEntryMatches(entry: LauncherEntry, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (!needle) return true;
  if (entry.searchText.includes(needle)) return true;
  return isSubsequence(needle.replace(/\s+/g, ""), entry.id.toLowerCase());
}

function searchTextFor(entry: Omit<LauncherEntry, "searchText">): string {
  return [
    entry.id,
    entry.label,
    entry.description,
    entry.category,
    entry.kind,
    entry.source,
    entry.service,
    entry.action,
    "upstream" in entry ? entry.upstream : "",
  ]
    .join(" ")
    .toLowerCase();
}

function isSubsequence(needle: string, haystack: string): boolean {
  let index = 0;
  for (const ch of haystack) {
    if (ch === needle[index]) index += 1;
    if (index === needle.length) return true;
  }
  return needle.length === 0;
}

export interface LauncherCatalogState {
  actions: LauncherEntry[];
  loading: boolean;
  error: string | null;
  refresh: () => void;
}

export function useLauncherCatalog(): LauncherCatalogState {
  const [actions, setActions] = useState<LauncherEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const etagRef = useRef<string | null>(null);
  const [tick, setTick] = useState(0);

  useEffect(() => {
    let active = true;
    setLoading(true);
    fetchLauncherCatalog(etagRef.current)
      .then((result) => {
        if (!active) return;
        if ("ok" in result) {
          setError(resultErrorMessage(result));
          return;
        }
        if (result.notModified) {
          setError(null);
          return;
        }
        setActions(normalizeLauncherCatalog(result.catalog));
        setError(null);
      })
      .catch((err) => {
        if (!active) return;
        setError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [tick]);

  const refresh = useCallback(() => setTick((value) => value + 1), []);

  return { actions, loading, error, refresh };
}
