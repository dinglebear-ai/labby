/**
 * Feature-gated capability derivation for the admin UI.
 *
 * The `labby` binary can be compiled without certain services (`acp`, `nodes`,
 * `stash`, `marketplace`). When a service is gated out at compile time it never
 * registers, so it is absent from `/v1/catalog`. The UI reads that same catalog
 * to decide which feature-specific pages and nav entries to show, so a
 * gateway-only build hides Chat/Nodes/Marketplace instead of letting those
 * pages fail with an opaque `404` from their missing `/v1/*` routes.
 *
 * `deriveCapabilities` is a pure function over the catalog service list so it
 * can be unit-tested without React; `useCapabilities` (in
 * `lib/hooks/use-capabilities.ts`) wraps it around the live catalog hook.
 */

/** Feature-gated capabilities the UI conditionally exposes. */
export interface Capabilities {
  /** ACP chat runtime (`/chat`). Backend service `acp`. */
  acp: boolean
  /** Fleet/node management (`/nodes`). Backend service `device`. */
  nodes: boolean
  /** Plugin/agent marketplace (`/marketplace`). Backend service `marketplace`. */
  marketplace: boolean
  /** Component snapshot store. Backend service `stash`. */
  stash: boolean
  /** True until the catalog has resolved at least once. */
  isLoading: boolean
}

/** Gated capability keys (excludes the always-true `isLoading` field). */
export type CapabilityKey = 'acp' | 'nodes' | 'marketplace' | 'stash'

/**
 * Backend service name backing each capability. Note the `nodes` capability is
 * backed by the service registered as `device` (the Cargo feature is `nodes`
 * but the registry name is `device`).
 */
export const CAPABILITY_SERVICE: Record<CapabilityKey, string> = {
  acp: 'acp',
  nodes: 'device',
  marketplace: 'marketplace',
  stash: 'stash',
}

/** Minimal shape `deriveCapabilities` needs from a catalog service entry. */
interface NamedService {
  name: string
}

/**
 * Derive capability flags from the catalog service list.
 *
 * Fail-open: a capability is only reported ABSENT when the catalog has
 * confidently resolved (not loading, no error, non-empty) and omits its
 * backing service. While the catalog is loading, errored, or empty, every
 * capability is reported available so a transient fetch problem never hides a
 * surface that actually exists.
 */
export function deriveCapabilities(
  services: readonly NamedService[],
  isLoading: boolean,
  hasError: boolean,
): Capabilities {
  const confident = !isLoading && !hasError && services.length > 0
  const has = (name: string) => services.some((service) => service.name === name)
  const cap = (key: CapabilityKey) => !confident || has(CAPABILITY_SERVICE[key])

  return {
    acp: cap('acp'),
    nodes: cap('nodes'),
    marketplace: cap('marketplace'),
    stash: cap('stash'),
    isLoading,
  }
}
