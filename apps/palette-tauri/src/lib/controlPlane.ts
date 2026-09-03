import type { LauncherEntry } from "@/lib/launcherCatalog";
import { invoke } from "@/lib/invoke";

const SERVICE_PATHS: Record<string, string> = {
  artifacts: "/skills/",
  bundles: "/skills/",
  doctor: "/settings/doctor/",
  fs: "/tools/",
  gateway: "/gateways/",
  jobs: "/skills/",
  lab_admin: "/settings/core/",
  server_logs: "/traces/",
  setup: "/settings/",
  snippets: "/snippets/",
  sources: "/skills/",
  uploads: "/skills/",
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
