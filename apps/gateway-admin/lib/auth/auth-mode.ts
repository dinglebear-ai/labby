export function hasApiTokenAuth(_token?: string) {
  void _token
  return false
}

export function isStandaloneBearerAuthMode(
  _token?: string,
) {
  void _token
  return false
}

export function hasMockDataAuthMode(mockData = process.env.NEXT_PUBLIC_MOCK_DATA) {
  return mockData === 'true'
}

export function shouldBypassBrowserSessionAuth(
  _token?: string,
  mockData = process.env.NEXT_PUBLIC_MOCK_DATA,
  nodeEnv = process.env.NODE_ENV,
) {
  void _token
  return nodeEnv === 'development' || hasMockDataAuthMode(mockData)
}
