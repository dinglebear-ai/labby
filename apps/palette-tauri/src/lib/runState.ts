import type { PaletteResult } from "@/lib/labbyClient";

// Generic run lifecycle for a dispatched action. No job/streaming variants —
// Labby's dispatch is request/response (see the plan's deferred-scope note on
// SSE/async job families).
export type RunState =
  | { kind: "idle" }
  | { kind: "running"; title: string }
  | { kind: "success"; title: string; result: PaletteResult }
  | { kind: "error"; title: string; result: PaletteResult; message: string };
