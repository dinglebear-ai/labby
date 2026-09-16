'use client'

import { Loader2 } from 'lucide-react'
import { toast } from 'sonner'

import { AURORA_DENSE_META } from '@/components/aurora/tokens'
import { DashboardPanel } from '@/components/dashboard/panel'
import { Button } from '@/components/ui/button'
import { shouldBypassBrowserSessionAuth } from '@/lib/auth/auth-mode'
import { selectSessionWorkspace, type BrowserSessionState } from '@/lib/auth/session'
import { cn, getErrorMessage } from '@/lib/utils'

/**
 * The project a browser session is bound to, if any.
 *
 * A fresh session always starts in the Personal workspace with no project,
 * and the server refuses every project-scoped request (`artifacts.*` and the
 * remote control-plane services) that arrives without one. Project-scoped
 * pages mount their content only when this is set instead of issuing reads
 * that can only be refused.
 */
export function sessionProjectId(session: BrowserSessionState): string | undefined {
  return session.status === 'authenticated' ? session.projectId : undefined
}

/**
 * Terminal states for a project-scoped page whose session has no project:
 * waits for the session, explains mock data mode, offers the projects the
 * server projected for this caller, or asks the operator to sign in.
 *
 * Choosing a project goes through the shared workspace switch, so the
 * sidebar, request headers, and every other page observe the same context.
 * The choice is a selector only; the server revalidates membership on every
 * request.
 */
export function ProjectWorkspaceRequired({ session, description }: { session: BrowserSessionState; description: string }) {
  if (session.status === 'loading' && !shouldBypassBrowserSessionAuth()) {
    return <div className="flex min-h-56 items-center justify-center"><Loader2 className="size-5 animate-spin" /></div>
  }
  if (session.status === 'loading') {
    return (
      <DashboardPanel title="Project required">
        <p className="text-sm text-aurora-text-muted">Mock data mode does not project an authenticated project. Use a live project-bound session to continue.</p>
      </DashboardPanel>
    )
  }
  if (session.status !== 'authenticated') {
    return (
      <DashboardPanel title="Project required">
        <p className="text-sm text-destructive">{session.status === 'auth_error' ? session.message : 'Sign in to select a project workspace.'}</p>
      </DashboardPanel>
    )
  }
  const projects = session.authority?.projects ?? []
  const chooseProject = (projectId: string) => {
    try {
      selectSessionWorkspace({ projectId })
    } catch (cause) {
      toast.error(getErrorMessage(cause, 'The project workspace could not be selected.'))
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
        <p className={cn(AURORA_DENSE_META, 'mt-3 text-aurora-text-muted')}>No eligible project is available for this session. Create or assign a project in the Control Plane, then refresh your session.</p>
      )}
    </DashboardPanel>
  )
}
