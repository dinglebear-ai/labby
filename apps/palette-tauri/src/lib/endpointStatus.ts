export type EndpointTone = "error" | "syncing" | "online";

export interface EndpointStatusInput {
  configured: boolean;
  catalogLoading: boolean;
  configError: string | null;
  catalogError: string | null;
}

interface EndpointStatusMessageInput extends EndpointStatusInput {
  endpointLabel: string;
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

function compactError(error: string): string {
  return error.replace(/\s+/g, " ").trim().slice(0, 180);
}

export function endpointStatusMessage({
  endpointLabel,
  configured,
  catalogLoading,
  configError,
  catalogError,
}: EndpointStatusMessageInput): string {
  if (configError)
    return `Configuration unavailable: ${compactError(configError)}. Restart Labby Palette and try again.`;
  if (catalogError)
    return `Catalog unavailable: ${compactError(catalogError)}. Check the server connection in Settings.`;
  if (!configured) return "Loading server configuration.";
  if (catalogLoading) return `Syncing the catalog from ${endpointLabel}.`;
  return `Connected to ${endpointLabel}.`;
}
