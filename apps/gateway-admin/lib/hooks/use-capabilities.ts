/**
 * React hook exposing feature-gated capabilities derived from `/v1/catalog`.
 *
 * Reuses the same catalog fetch as the ⌘K palette (`useCommandCatalog`) so no
 * extra request is made. See `lib/capabilities.ts` for the pure derivation and
 * fail-open semantics.
 */

import { useCommandCatalog } from '@/lib/hooks/use-command-catalog'
import { deriveCapabilities, type Capabilities } from '@/lib/capabilities'

/** Live feature-gated capabilities for the current server build. */
export function useCapabilities(): Capabilities {
  const { data, error } = useCommandCatalog()
  return deriveCapabilities(data, error != null)
}
