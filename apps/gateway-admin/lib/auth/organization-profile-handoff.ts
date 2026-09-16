import type { OrganizationBootstrapCreateResponse } from '../api/setup-client.ts'

export const ORGANIZATION_PROFILE_OFFER_SESSION_KEY = 'labby.organization-bootstrap.offer'
export const ORGANIZATION_PROFILE_IMPORT_SESSION_KEY = 'labby.organization-bootstrap.import'
export const ORGANIZATION_PROFILE_FRAGMENT_KEY = 'organization-bootstrap'

const SHA256_HEX_RE = /^[0-9a-f]{64}$/i

export function normalizePersonalLabbyOrigin(raw: string): string | undefined {
  const input = raw.trim()
  if (!input) return undefined
  let url: URL
  try {
    url = new URL(input)
  } catch {
    return undefined
  }
  const loopback = url.hostname === '127.0.0.1' || url.hostname === 'localhost' || url.hostname === '::1'
  if ((url.protocol !== 'https:' && !(loopback && url.protocol === 'http:')) ||
      url.username || url.password || url.pathname !== '/' || url.search || url.hash) {
    return undefined
  }
  return url.origin
}

export function normalizeOrganizationProfileOffer(value: unknown): OrganizationBootstrapCreateResponse | undefined {
  if (!isRecord(value)) return undefined
  const fingerprint = typeof value.signer_fingerprint === 'string' ? value.signer_fingerprint.trim().toLowerCase() : ''
  if (!SHA256_HEX_RE.test(fingerprint) || !isRecord(value.profile)) return undefined
  const profile = value.profile
  if (
    typeof profile.schema_version !== 'string' ||
    typeof profile.organization_id !== 'string' ||
    typeof profile.issued_at !== 'number' ||
    typeof profile.key_id !== 'string' ||
    typeof profile.verifying_key !== 'string' ||
    !Array.isArray(profile.integrations) ||
    typeof profile.signature !== 'string'
  ) {
    return undefined
  }
  return {
    profile: value.profile as unknown as OrganizationBootstrapCreateResponse['profile'],
    signer_fingerprint: fingerprint,
  }
}

export function parseOrganizationProfileOfferJson(raw: string | null | undefined): OrganizationBootstrapCreateResponse | undefined {
  if (!raw) return undefined
  try {
    return normalizeOrganizationProfileOffer(JSON.parse(raw))
  } catch {
    return undefined
  }
}

export function organizationProfileOfferFromHash(hash: string): OrganizationBootstrapCreateResponse | undefined {
  if (!hash.startsWith('#')) return undefined
  const params = new URLSearchParams(hash.slice(1))
  return parseOrganizationProfileOfferJson(params.get(ORGANIZATION_PROFILE_FRAGMENT_KEY))
}

export function organizationProfileHandoffHref(
  personalLabbyOrigin: string,
  offer: OrganizationBootstrapCreateResponse,
): string {
  const origin = normalizePersonalLabbyOrigin(personalLabbyOrigin)
  const normalized = normalizeOrganizationProfileOffer(offer)
  if (!origin) throw new Error('Personal Labby URL must be an HTTPS origin or a loopback HTTP origin.')
  if (!normalized) throw new Error('Organization bootstrap offer is malformed.')
  const params = new URLSearchParams()
  params.set(ORGANIZATION_PROFILE_FRAGMENT_KEY, JSON.stringify(normalized))
  return origin + '/#' + params.toString()
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}
