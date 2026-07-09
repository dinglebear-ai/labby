// Thin fetch wrapper over the Rust bridge for a `labby serve` instance:
//   - fetchCatalog()  → invoke("fetch_catalog")   → GET  /v1/catalog
//   - dispatchAction() → invoke("dispatch_action") → POST /v1/{service}
// All HTTP goes through the shared invoke seam (Tauri IPC in production), so the
// server URL + auth token are resolved by the Rust shell, never in the renderer.
import { invoke } from "./invoke";

export interface PaletteConfig {
  serverUrl: string;
  staticToken?: string | null;
  shortcut: string;
  theme: "system" | "dark" | "light";
  hideOnBlur: boolean;
  openResultsInline?: boolean;
  showFooterHints?: boolean;
}

export interface PaletteResult {
  ok: boolean;
  status: number;
  path: string;
  method: string;
  payload: unknown;
}

export interface LabbyActionEntry {
  name: string;
  description: string;
  destructive: boolean;
  params: { name: string; ty: string; required: boolean; description: string }[];
  returns: string;
}

export interface LabbyServiceCatalog {
  name: string;
  description: string;
  category: string;
  status: string;
  actions: LabbyActionEntry[];
}

export interface LabbyCatalog {
  services: LabbyServiceCatalog[];
}

// The Rust `fetch_catalog` command returns `{ ok, status, payload }`. A 304
// answer carries `status: 304` with a null payload; a 200 carries the parsed
// catalog as its payload.
interface BridgeResult {
  ok: boolean;
  status: number;
  payload: unknown;
}

export type CatalogResult =
  | { notModified: true }
  | { notModified: false; catalog: LabbyCatalog };

/**
 * Fetch the Labby action catalog. Passes `etag` as `If-None-Match`; a `304`
 * response resolves to `{ notModified: true }`. A non-success HTTP status
 * rejects with a descriptive error.
 */
export async function fetchCatalog(etag?: string | null): Promise<CatalogResult> {
  const result = await invoke<BridgeResult>("fetch_catalog", { etag: etag ?? null });
  if (result.status === 304) return { notModified: true };
  if (!result.ok) {
    throw new Error(`Catalog request failed with HTTP ${result.status}.`);
  }
  return { notModified: false, catalog: (result.payload ?? { services: [] }) as LabbyCatalog };
}

/**
 * Dispatch `{ action, params }` to `POST /v1/{service}`. HTTP-level failures
 * (4xx/5xx) resolve to a `PaletteResult` with `ok: false`; only a network-level
 * failure rejects.
 */
export async function dispatchAction(
  service: string,
  action: string,
  params: unknown,
): Promise<PaletteResult> {
  const result = await invoke<BridgeResult>("dispatch_action", {
    request: { service, action, params },
  });
  return {
    ok: result.ok,
    status: result.status,
    path: `/v1/${service}`,
    method: "POST",
    payload: result.payload,
  };
}

/**
 * Extract a human-readable message from a failed dispatch. Probes the stable
 * Labby error envelope (`{ kind, message, … }`, see docs/dev/ERRORS.md), then
 * common alternatives, falling back to the HTTP status line.
 */
export function resultErrorMessage(result: PaletteResult): string {
  const payload = result.payload;
  if (payload && typeof payload === "object" && !Array.isArray(payload)) {
    const record = payload as Record<string, unknown>;
    for (const key of ["message", "error", "detail"]) {
      const value = record[key];
      if (typeof value === "string" && value.trim()) return value;
    }
  }
  return `Request failed with HTTP ${result.status || "local"}.`;
}
