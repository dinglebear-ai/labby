'use client'

import { useCallback, useEffect, useState } from 'react'
import { Archive, CirclePlus, FolderKanban, RefreshCw } from 'lucide-react'

import { AppHeader } from '@/components/app-header'
import { AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { ConsoleHero } from '@/components/console/console-hero'
import { DashboardPanel } from '@/components/dashboard/panel'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { isAbortError } from '@/lib/api/service-action-client'
import { authorityIdentity, useBrowserSession } from '@/lib/auth/session'
import { archiveProject, createProject, listProjects, type ProjectView } from '@/lib/projects/client'

function failureMessage(reason: unknown, fallback: string) {
  return reason instanceof Error ? reason.message : fallback
}

export function ProjectsPageContent() {
  const session = useBrowserSession()
  const authority = session.status === 'authenticated' ? session.authority : undefined
  const workspaceIdentity = authorityIdentity(authority)
  // Subscribed through the session store so a workspace switch re-renders the
  // form and the create gate instead of reading a one-shot snapshot.
  const team = authority?.activeTeamId
  const [rows, setRows] = useState<ProjectView[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string>()
  const [busy, setBusy] = useState<string>()
  const [id, setId] = useState('')
  const [name, setName] = useState('')

  const load = useCallback(async (signal?: AbortSignal) => {
    setLoading(true)
    setError(undefined)
    try {
      setRows(await listProjects(signal))
    } catch (reason) {
      if (!isAbortError(reason)) setError(failureMessage(reason, 'Projects unavailable'))
    } finally {
      if (!signal?.aborted) setLoading(false)
    }
  }, [])

  useEffect(() => {
    const controller = new AbortController()
    void load(controller.signal)
    return () => controller.abort()
  }, [load, workspaceIdentity])

  const create = async () => {
    if (!team) {
      setError('Select a Team before creating a Project.')
      return
    }
    setBusy('create')
    setError(undefined)
    try {
      await createProject(team, id.trim(), name.trim())
      setId('')
      setName('')
      await load()
    } catch (reason) {
      if (!isAbortError(reason)) setError(failureMessage(reason, 'Create failed'))
    } finally {
      setBusy(undefined)
    }
  }

  const archive = async (row: ProjectView) => {
    const key = `${row.team_id}:${row.project_id}`
    setBusy(key)
    setError(undefined)
    try {
      await archiveProject(row.team_id, row.project_id)
      await load()
    } catch (reason) {
      if (!isAbortError(reason)) setError(failureMessage(reason, 'Archive failed'))
    } finally {
      setBusy(undefined)
    }
  }

  const canCreate = Boolean(team) && id.trim().length > 0 && name.trim().length > 0 && busy !== 'create'

  return (
    <>
      <AppHeader breadcrumbs={[{ label: 'Workspace' }, { label: 'Projects' }]} />
      <div className={`${AURORA_PAGE_SHELL} flex-1`}>
        <main className={`${AURORA_PAGE_FRAME} space-y-4`}>
          <ConsoleHero
            eyebrow={`Workspace · ${authority?.activeOwner.kind ?? 'unavailable'}`}
            title="Projects"
            description="Team-assigned Projects from Labby’s authority store."
            pulse={{ color: error ? 'var(--aurora-error)' : 'var(--aurora-success)' }}
            actions={<Button variant="outline" onClick={() => void load()} disabled={loading}><RefreshCw />Refresh</Button>}
            stats={[{ label: 'Visible', value: loading ? '—' : rows.length, icon: <FolderKanban size={12} /> }]}
          />
          {error ? <div role="alert" className="rounded-aurora-2 border border-aurora-error/35 bg-aurora-error/5 p-4 text-sm text-aurora-error">{error}</div> : null}
          <DashboardPanel title="Create a Project">
            {team ? (
              <div className="grid gap-3 md:grid-cols-[1fr_1fr_auto]">
                <Input aria-label="Project ID" placeholder="project-id" value={id} onChange={(event) => setId(event.target.value)} />
                <Input aria-label="Project name" placeholder="Project name" value={name} onChange={(event) => setName(event.target.value)} />
                <Button onClick={() => void create()} disabled={!canCreate}><CirclePlus />Create Project</Button>
              </div>
            ) : (
              <p className="text-sm text-aurora-text-muted">Select a Team workspace to create a Project.</p>
            )}
          </DashboardPanel>
          <DashboardPanel title="Projects">
            {loading ? (
              <p role="status" className="py-8 text-center text-sm text-aurora-text-muted">Loading accessible Projects…</p>
            ) : rows.length === 0 ? (
              <p className="py-8 text-center text-sm text-aurora-text-muted">No accessible Projects.</p>
            ) : (
              <div className="divide-y divide-aurora-border-subtle">
                {rows.map((row) => {
                  const key = `${row.team_id}:${row.project_id}`
                  return (
                    <article key={key} className="flex items-center justify-between gap-4 py-4">
                      <div className="min-w-0">
                        <strong className="block truncate text-sm text-aurora-text-primary">{row.name}</strong>
                        <p className="text-xs text-aurora-text-muted">{row.project_id} · {row.team_id} · {row.role} · policy {row.policy_epoch}</p>
                      </div>
                      <Button variant="outline" size="sm" aria-label={`Archive ${row.name}`} disabled={!row.can_manage || busy === key} onClick={() => void archive(row)}><Archive />Archive</Button>
                    </article>
                  )
                })}
              </div>
            )}
          </DashboardPanel>
        </main>
      </div>
    </>
  )
}
