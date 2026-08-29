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
