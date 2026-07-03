import { CircleAlert, Loader2 } from 'lucide-react'

/**
 * Brief centered spinner shown while the catalog has not yet resolved
 * (`!capabilities.ready`). Rendered by `CapabilityGuard` INSTEAD OF the guarded
 * children so their `/v1/*` fetches do not fire before the catalog confirms the
 * backing service exists.
 */
export function CapabilityPending() {
  return (
    <div className="flex min-h-[60vh] items-center justify-center">
      <Loader2 className="size-6 animate-spin text-aurora-text-muted" />
    </div>
  )
}

/**
 * Full-page state shown when a feature-gated service was not compiled into this
 * `labby` build, so its page has no backing `/v1/*` routes. Replaces the opaque
 * `404` a gated page would otherwise hit.
 */
export function ServiceUnavailableNotice({ serviceName }: { serviceName: string }) {
  return (
    <div className="flex min-h-[60vh] flex-col items-center justify-center gap-3 px-6 text-center">
      <CircleAlert className="size-10 text-aurora-warn" />
      <h2 className="text-lg font-semibold text-aurora-text-primary">
        {serviceName} is not available in this build
      </h2>
      <p className="max-w-md text-sm text-aurora-text-muted">
        This Labby binary was compiled without the {serviceName.toLowerCase()} service, so this page
        has nothing to show. Rebuild with the corresponding feature enabled to use it.
      </p>
    </div>
  )
}
