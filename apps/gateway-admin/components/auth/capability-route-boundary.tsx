'use client'

import { usePathname } from 'next/navigation'

import { AURORA_COMPACT_TITLE, AURORA_MUTED_LABEL } from '@/components/aurora/tokens'
import { capabilityForPath } from '@/components/console/nav-model'
import { authorityIdentity, useBrowserSession } from '@/lib/auth/session'

export function CapabilityRouteBoundary({ children }: { children: React.ReactNode }) {
  const pathname = usePathname()
  const session = useBrowserSession()
  if (session.status !== 'authenticated') return children
  const required = capabilityForPath(pathname)
  const allowed = required === null || (required !== undefined && session.authority?.capabilities.includes(required))
  if (allowed) {
    // Remounting on the shared authority identity discards page state that was
    // loaded under a previous workspace, principal, or capability set. A
    // missing projection is its own identity, so gaining or losing one also
    // remounts.
    return <div className="contents" key={authorityIdentity(session.authority)}>{children}</div>
  }
  return (
    <main className="mx-auto flex min-h-[60vh] max-w-xl items-center px-6">
      <section role="alert" className="w-full rounded-xl border border-aurora-border-default bg-aurora-panel-medium p-6">
        <p className={AURORA_MUTED_LABEL}>Workspace access</p>
        <h1 className={`${AURORA_COMPACT_TITLE} mt-2 text-aurora-text-primary`}>This page is not available in the selected workspace</h1>
        <p className="mt-2 text-sm text-aurora-text-secondary">Choose another Team, Project, or Personal workspace, or ask an owner to update your access.</p>
      </section>
    </main>
  )
}
