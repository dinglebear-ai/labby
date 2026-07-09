// Generic, catalog-driven palette action model. Replaces Axon's hand-maintained
// static ACTIONS/actionRegistry: a PaletteAction is derived at runtime from
// Labby's self-describing `GET /v1/catalog` (see actionCatalog.ts).

export interface ParamEntry {
  name: string;
  ty: string;
  required: boolean;
  description: string;
}

export type ArgMode = "none" | "text" | "json";

export interface PaletteAction {
  /** "service.action" dotted id — the stable key for selection/list rows. */
  subcommand: string;
  service: string;
  action: string;
  /** Human display, e.g. "radarr: movie.search". */
  label: string;
  description: string;
  category: string;
  destructive: boolean;
  params: ParamEntry[];
  /** Derived from params: "none" when empty, "json" for object/array params, else "text". */
  argMode: ArgMode;
}

/** Derive the argument-entry mode from an action's parameter shapes. */
export function deriveArgMode(params: ParamEntry[]): ArgMode {
  if (params.length === 0) return "none";
  const hasStructured = params.some((param) => {
    const ty = param.ty.toLowerCase();
    return ty.includes("object") || ty.includes("array") || ty.includes("map") || ty.includes("[");
  });
  return hasStructured ? "json" : "text";
}

/** Case-insensitive fuzzy/substring match over label, description, and subcommand. */
export function actionMatches(action: PaletteAction, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (!needle) return true;
  const haystack = `${action.subcommand} ${action.label} ${action.description} ${action.category}`.toLowerCase();
  if (haystack.includes(needle)) return true;
  // Loose subsequence match on the subcommand so "rms" matches "radarr.movie.search".
  return isSubsequence(needle.replace(/\s+/g, ""), action.subcommand.toLowerCase());
}

function isSubsequence(needle: string, haystack: string): boolean {
  let index = 0;
  for (const ch of haystack) {
    if (ch === needle[index]) index += 1;
    if (index === needle.length) return true;
  }
  return needle.length === 0;
}
