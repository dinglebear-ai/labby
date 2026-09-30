'use client'

import { toast } from 'sonner'

import { AURORA_DENSE_META } from '@/components/aurora/tokens'
import { DashboardPanel } from '@/components/dashboard/panel'
import { Button } from '@/components/ui/button'
import { shouldBypassBrowserSessionAuth } from '@/lib/auth/auth-mode'
import { WorkspaceSelectionError, selectSessionWorkspace, useBrowserSession } from '@/lib/auth/session'
import { cn } from '@/lib/utils'

/**
 * What a project-scoped page renders while its session has no project.
 *
 * An OAuth or bearer sign-in starts in the Personal workspace with no project
 * selected; only a source-bound project session arrives already bound. The
 * Skills and Depot Library pages mount their collections only for a bound
 * session (`useProjectBoundSessionScope`) and render this otherwise, because
 * the server refuses every `artifacts.*` action that arrives without
 * `x-labby-project-id`.
 *
 * Two states reach operators: mock data mode, where the session never loads
 * (the Depot Library mounts its preview collection instead of rendering
 * this), and an authenticated session with no project, which is offered the
 * projects the server projected for this caller. `AuthBootstrap` owns the
 * loading, signed-out, and auth-error states in the admin layout, so those
 * render nothing here.
 *
 * Choosing a project goes through `selectSessionWorkspace`, the same switch
 * the sidebar uses, so `gatewayHeaders` and every page keyed on the session
 * scope observe the same context. The client only accepts a project the
 * server projected; the server re-authorizes membership on every request.
 */
export function ProjectWorkspaceRequired({ description }: { description: string }) {
  const session = useBrowserSession()
  if (session.status === 'loading' && shouldBypassBrowserSessionAuth()) {
    return (
      <DashboardPanel title="Project required">
        <p className="text-sm text-aurora-text-muted">Mock data mode does not project an authenticated project. Use a live project-bound session to continue.</p>
      </DashboardPanel>
    )
  }
  if (session.status !== 'authenticated') return null
  if (!session.authority) {
    return (
      <DashboardPanel title="Project required">
        <p className="text-sm text-aurora-text-muted">This session carries no workspace authority projection, so no project can be selected. Sign in again.</p>
      </DashboardPanel>
    )
  }
  const projects = session.authority.projects
  const chooseProject = (projectId: string) => {
    try {
      selectSessionWorkspace({ projectId })
    } catch (cause) {
      if (!(cause instanceof WorkspaceSelectionError)) throw cause
      toast.error(cause.message)
    }
  }
  return (
    <DashboardPanel title="Project required">
      <p className="text-sm text-aurora-text-muted">{description}</p>
      {projects.length > 0 ? (
        <div className="mt-4 flex flex-wrap gap-2">
          {projects.map(project => (
            <Button key={project.id} variant="outline" size="sm" onClick={() => chooseProject(project.id)}>
              {project.name ?? project.id}
            </Button>
          ))}
        </div>
      ) : (
        <p className={cn(AURORA_DENSE_META, 'mt-3 text-aurora-text-muted')}>No eligible project is available for this session. Create or assign a project in the Control Plane, then reload the page.</p>
      )}
    </DashboardPanel>
  )
}
