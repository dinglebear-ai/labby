export function isGatewayURL(url, expectedOrigin) {
  return url.origin === expectedOrigin && url.pathname === '/gateway'
}
