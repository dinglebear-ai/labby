/**
 * Feature-gated capability derivation for the admin UI.
 *
 * The `labby` binary can be compiled without certain services (`acp`, `nodes`,
 * `marketplace`). When a service is gated out at compile time it never
 * registers, so it is absent from `/v1/catalog`. The UI reads that same catalog
 * to decide which feature-specific pages and nav entries to show, so a
 * gateway-only build hides Chat/Nodes/Marketplace instead of letting those
 * pages fail with an opaque `404` from their missing `/v1/*` routes.
 *
 * `deriveCapabilities` is a pure function over the catalog service list so it
 * can be unit-tested without React; `useCapabilities` (in
 * `lib/hooks/use-capabilities.ts`) wraps it around the live catalog hook.
 *
 * Fail-open is keyed on whether the catalog has RESOLVED — i.e. real data has
 * arrived (`services.length > 0`) or the fetch errored — not on a loading flag.
 * The catalog SWR hook sets `fallbackData: []`, so `isLoading` is already
 * `false` on the first render even though no data has arrived yet; `ready`
 * captures the "definitive answer received" condition that `isLoading` cannot.
 */

/** Feature-gated capabilities the UI conditionally exposes. */
export interface Capabilities {
  /** ACP chat runtime (`/chat`). Backend service `acp`. */
  acp: boolean
  /** Fleet/node management (`/nodes`). Backend service `device`. */
  nodes: boolean
  /** Plugin/agent marketplace (`/marketplace`). Backend service `marketplace`. */
  marketplace: boolean
  /**
   * True once the catalog has given a definitive answer — either real data has
   * arrived (`services.length > 0`) or the fetch errored. Until then callers
   * must not treat an absent capability as confirmed: the per-capability
   * booleans stay fail-open (available) while `!ready`, but consumers that fire
   * `/v1/*` requests (guarded pages, the chat bootstrap, the dashboard nodes
   * tile) should wait for `ready` before fetching so a gated build never emits
   * the background `404` we are avoiding.
   */
  ready: boolean
}

/** Gated capability keys (excludes the `ready` meta-field). */
export type CapabilityKey = 'acp' | 'nodes' | 'marketplace'

/**
 * Backend service name backing each capability. Note the `nodes` capability is
 * backed by the service registered as `device` (the Cargo feature is `nodes`
 * but the registry name is `device`).
 */
export const CAPABILITY_SERVICE: Record<CapabilityKey, string> = {
  acp: 'acp',
  nodes: 'device',
  marketplace: 'marketplace',
}

/** Minimal shape `deriveCapabilities` needs from a catalog service entry. */
interface NamedService {
  name: string
}

/**
 * Derive capability flags from the catalog service list.
 *
 * Fail-open: a capability is only reported ABSENT when the catalog has given a
 * definitive answer (`ready`) and omits its backing service. While the catalog
 * has not resolved (no data yet, empty due to `fallbackData: []`), every
 * capability is reported available so a transient fetch state never hides a
 * surface that actually exists. On error the catalog is also treated as
 * definitive-but-fail-open: `ready` is true, but every capability stays
 * available so a fetch failure never hides a working surface.
 */
export function deriveCapabilities(
  services: readonly NamedService[],
  hasError: boolean,
): Capabilities {
  // `ready` = the catalog has given a definitive answer. A loading flag can't
  // drive this: the catalog SWR hook sets `fallbackData: []`, so `isLoading` is
  // already false on the first render before any data has arrived — it cannot
  // distinguish "resolved to empty" from "not resolved yet". `services.length`
  // can (a running gateway always registers gateway/doctor/setup).
  const ready = hasError || services.length > 0
  const has = (name: string) => services.some((service) => service.name === name)
  const cap = (key: CapabilityKey) => !ready || hasError || has(CAPABILITY_SERVICE[key])

  return {
    acp: cap('acp'),
    nodes: cap('nodes'),
    marketplace: cap('marketplace'),
    ready,
  }
}

/**
 * True only once the catalog has confirmed the backing service is present
 * (`ready && caps[key]`). Consumers that fire `/v1/*` requests — guarded pages,
 * the chat bootstrap, the dashboard nodes tile — gate on this so a gated build
 * never emits the background `404` we are avoiding, and so they also hold off
 * during the brief pre-resolution window rather than fetching optimistically.
 *
 * Safe to gate fetches on because the catalog resolves exactly once:
 * `useCommandCatalog` disables revalidate-on-stale/focus/reconnect, so `ready`
 * goes false→true a single time and never flaps back (which would otherwise
 * re-fire these gated fetches).
 */
export function capabilityAvailable(caps: Capabilities, key: CapabilityKey): boolean {
  return caps.ready && caps[key]
}
