// Fetches Labby's `GET /v1/catalog` once and flattens
// `services[].actions[]` into a flat PaletteAction[] for the command palette.
// Stores the ETag for revalidation (a 304 keeps the current list).
import { useCallback, useEffect, useRef as _useRef, useState } from "react";

import { deriveArgMode, type PaletteAction } from "@/lib/actions";
import { fetchCatalog, type LabbyCatalog } from "@/lib/labbyClient";

export function flattenCatalog(catalog: LabbyCatalog): PaletteAction[] {
  const actions: PaletteAction[] = [];
  for (const service of catalog.services ?? []) {
    for (const entry of service.actions ?? []) {
      const params = entry.params ?? [];
      actions.push({
        subcommand: `${service.name}.${entry.name}`,
        service: service.name,
        action: entry.name,
        label: `${service.name}: ${entry.name}`,
        description: entry.description,
        category: service.category,
        destructive: entry.destructive,
        params,
        argMode: deriveArgMode(params),
      });
    }
  }
  return actions;
}

export interface ActionCatalog {
  actions: PaletteAction[];
  loading: boolean;
  error: string | null;
  refresh: () => void;
}

export function useActionCatalog(): ActionCatalog {
  const [actions, setActions] = useState<PaletteAction[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const etagRef = _useRef<string | null>(null);
  const [tick, setTick] = useState(0);

  // biome-ignore lint/correctness/useExhaustiveDependencies: `tick` is a refresh trigger, not read in the body.
  useEffect(() => {
    let active = true;
    setLoading(true);
    fetchCatalog(etagRef.current)
      .then((result) => {
        if (!active) return;
        if (result.notModified) {
          setError(null);
          return;
        }
        setActions(flattenCatalog(result.catalog));
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
