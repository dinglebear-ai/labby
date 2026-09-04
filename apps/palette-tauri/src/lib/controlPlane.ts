import { invoke } from "@/lib/invoke";
import type { LauncherEntry } from "@/lib/launcherCatalog";

const SERVICE_PATHS: Record<string, string> = {
  artifacts: "/skills/?view=control-plane",
  bundles: "/skills/?view=control-plane",
  doctor: "/settings/doctor/",
  fs: "/tools/",
  gateway: "/gateways/",
  jobs: "/skills/?view=control-plane",
  lab_admin: "/settings/core/",
  server_logs: "/traces/",
  setup: "/settings/",
  snippets: "/snippets/",
  sources: "/skills/?view=control-plane",
  uploads: "/skills/?view=control-plane",
};

export function controlPlanePath(entry?: LauncherEntry): string {
  if (!entry) return "/";
  if (entry.id.startsWith("mcp:")) return "/tools/";
  const match = /^labby:([^:]+)::/.exec(entry.id);
  return match ? (SERVICE_PATHS[match[1]] ?? "/") : "/";
}

export function openControlPlane(path = "/") {
  return invoke<void>("open_control_plane", { path });
}

export function controlPlaneErrorMessage(error: unknown): string {
  const detail = error instanceof Error ? error.message : String(error);
  return `Could not open the Control Plane: ${detail}`;
}

let launchGeneration = 0;

export async function launchControlPlane(
  path: string,
  onError: (message: string) => void,
): Promise<void> {
  const generation = ++launchGeneration;
  try {
    await openControlPlane(path);
  } catch (error) {
    if (generation === launchGeneration) {
      onError(controlPlaneErrorMessage(error));
    }
  }
}
