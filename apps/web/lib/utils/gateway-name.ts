/**
 * Shared gateway name validation.
 *
 * The Rust backend requires only that the name is non-empty.
 * This client-side rule is stricter to guide users toward names that are
 * portable across all surfaces (CLI flags, env var derivation, file paths):
 *   - starts with a letter or digit
 *   - contains only letters, digits, hyphens, or underscores
 *   - max 64 characters
 */
const GATEWAY_NAME_PATTERN = /^[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}$/
const BIDI_STRIP_RE = /[\u200B-\u200F\u202A-\u202E\u2060-\u2064\uFEFF]/g
const INVALID_CHARS_RE = /[^a-zA-Z0-9_-]/g

/**
 * Validate a gateway name.
 * Returns `null` when valid, or a human-readable error message when invalid.
 */
export function validateGatewayName(value: string): string | null {
  if (!value) return 'Gateway name is required'
  if (!GATEWAY_NAME_PATTERN.test(value))
    return 'Must start with a letter or digit and contain only letters, digits, underscores, or hyphens (max 64 chars)'
  return null
}

export function deriveGatewayName(serverName: string): string {
  const segment = serverName.split('/').at(-1) ?? serverName
  return segment
    .normalize('NFC')
    .replace(BIDI_STRIP_RE, '')
    .replace(INVALID_CHARS_RE, '')
    .slice(0, 64)
}
