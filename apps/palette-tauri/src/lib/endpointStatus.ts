export type EndpointTone = "error" | "syncing" | "online";

interface EndpointStatusInput {
  configured: boolean;
  catalogLoading: boolean;
  configError: string | null;
  catalogError: string | null;
}

export function endpointStatus({
  configured,
  catalogLoading,
  configError,
  catalogError,
}: EndpointStatusInput): EndpointTone {
  if (configError || catalogError) return "error";
  if (!configured || catalogLoading) return "syncing";
  return "online";
}

export function endpointStatusMessage({
  configured,
  catalogLoading,
  configError,
  catalogError,
  endpointLabel,
}: EndpointStatusInput & { endpointLabel: string }): string {
  if (configError) return "Configuration unavailable. Restart Labby Palette and try again.";
  if (catalogError) return "Catalog unavailable. Check the server connection in Settings.";
  if (!configured) return "Loading server configuration.";
  if (catalogLoading) return `Syncing the catalog from ${endpointLabel}.`;
  return `Connected to ${endpointLabel}.`;
}
