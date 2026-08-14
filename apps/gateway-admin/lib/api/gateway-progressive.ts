export type ProgressiveGatewayApi<TGateway> = {
  list: () => Promise<TGateway[]>
  hydrateRuntime: (gateways: TGateway[]) => Promise<TGateway[]>
}

export function loadGatewayConfiguration<TGateway>(
  api: ProgressiveGatewayApi<TGateway>,
): Promise<TGateway[]> {
  return api.list()
}

export function loadGatewayRuntime<TGateway>(
  api: ProgressiveGatewayApi<TGateway>,
  gateways: TGateway[],
): Promise<TGateway[]> {
  return api.hydrateRuntime(gateways)
}
