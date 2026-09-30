const ENV_VAR_NAME_PATTERN = /^[A-Za-z_][A-Za-z0-9_]*$/

function looksLikeRawBearerToken(value: string) {
  return (
    value.startsWith('Bearer ') ||
    value.startsWith('ghp_') ||
    value.startsWith('github_pat_') ||
    value.startsWith('ghu_') ||
    value.startsWith('ghs_') ||
    value.startsWith('ghr_')
  )
}

export function validateBearerTokenEnvName(value: string): string | null {
  const trimmed = value.trim()
  if (!trimmed) {
    return null
  }

  if (looksLikeRawBearerToken(trimmed)) {
    return 'Enter an environment variable name like GITHUB_MCP_PAT, not the token value itself.'
  }

  if (!ENV_VAR_NAME_PATTERN.test(trimmed)) {
    return 'Bearer token env var must be a valid environment variable name.'
  }

  return null
}

export function defaultGatewayBearerEnvName(name: string): string {
  const normalized = name
    .trim()
    .replace(/[^A-Za-z0-9]+/g, '_')
    .replace(/^_+|_+$/g, '')
    .replace(/_+/g, '_')
    .toUpperCase()

  return `LAB_GW_${normalized || 'GATEWAY'}_AUTH_HEADER`
}
