'use client'

import { useEffect, useRef, useState } from 'react'
import { AlertTriangle, Cherry, CheckCircle, Loader2, XCircle } from 'lucide-react'
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog'
import { cn } from '@/lib/utils'
import type { PluginComponent, PluginComponentKind } from '@/lib/types/marketplace'
import { getPluginComponents, cherryPickPlugin } from '@/lib/api/marketplace-client'
import { normalizeGatewayApiBase } from '@/lib/api/gateway-config'
import { isAbortError } from '@/lib/api/service-action-client'
import { DeviceSelector } from './device-selector'

// ── Types ──────────────────────────────────────────────────────────────────

export interface CherryPickDialogProps {
  pluginId: string
  pluginName: string
  open: boolean
  onClose: () => void
  /** Pre-loaded components; if absent they are fetched when the dialog opens */
  components?: Array<{ type: string; name: string; path?: string }>
}

interface InstallProgressEvent {
  type: 'progress' | 'complete' | 'error'
  rpc_id: string
  device_id: string
  status: 'pending' | 'installing' | 'done' | 'failed'
  file?: string
  error?: string
}

// ── Component group config (maps actual PluginComponentKind values) ────────

const COMPONENT_GROUPS: Array<{
  key: PluginComponentKind
  label: string
}> = [
  { key: 'agents', label: 'Agents' },
  { key: 'commands', label: 'Commands' },
  { key: 'skills', label: 'Skills' },
  { key: 'hooks', label: 'Hooks' },
  { key: 'apps', label: 'Apps' },
  { key: 'mcp_servers', label: 'MCP Servers' },
  { key: 'lsp_servers', label: 'LSP Servers' },
  { key: 'monitors', label: 'Monitors' },
  { key: 'channels', label: 'Channels' },
  { key: 'output_styles', label: 'Output Styles' },
  { key: 'themes', label: 'Themes' },
  { key: 'bin', label: 'Executables' },
  { key: 'settings', label: 'Settings' },
  { key: 'assets', label: 'Assets' },
  { key: 'files', label: 'Config Files' },
]

// ── Sub-components ─────────────────────────────────────────────────────────

function CheckIcon() {
  return (
    <svg className="w-2.5 h-2.5 text-white" fill="none" viewBox="0 0 10 8">
      <path d="M1 4l3 3 5-6" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  )
}

function ComponentCheckbox({
  id,
  label,
  path,
  checked,
  onChange,
}: {
  id: string
  label: string
  path?: string
  checked: boolean
  onChange: (id: string, checked: boolean) => void
}) {
  return (
    <label
      className={cn(
        'flex items-center gap-3 px-3 py-2 rounded-aurora-1 border cursor-pointer transition-colors',
        checked
          ? 'bg-[color-mix(in_srgb,var(--aurora-accent-primary)_8%,transparent)] border-[color-mix(in_srgb,var(--aurora-accent-primary)_30%,transparent)]'
          : 'bg-aurora-control-surface border-aurora-border-strong hover:bg-aurora-hover-bg',
      )}
    >
      <input
        type="checkbox"
        className="sr-only"
        checked={checked}
        onChange={(e) => onChange(id, e.target.checked)}
      />
      <div
        className={cn(
          'flex-shrink-0 w-4 h-4 rounded border flex items-center justify-center transition-colors',
          checked
            ? 'bg-aurora-accent-primary border-aurora-accent-primary'
            : 'bg-transparent border-aurora-border-strong',
        )}
      >
        {checked && <CheckIcon />}
      </div>
      <div className="flex-1 min-w-0">
        <div className="text-[13px] font-medium text-aurora-text-primary truncate">{label}</div>
        {path && (
          <div className="text-[11px] text-aurora-text-muted truncate font-mono">{path}</div>
        )}
      </div>
    </label>
  )
}

function DeviceProgressRow({ device_id, events }: { device_id: string; events: InstallProgressEvent[] }) {
  const latest = events[events.length - 1]
  const status = latest?.status ?? 'pending'
  const isDone = status === 'done'
  const isFailed = status === 'failed'
  const isInstalling = status === 'installing'

  return (
    <div className="flex flex-col gap-1 py-2 border-b border-aurora-border-default last:border-0">
      <div className="flex items-center gap-2">
        {isDone ? (
          <CheckCircle className="w-3.5 h-3.5 flex-shrink-0 text-aurora-success" />
        ) : isFailed ? (
          <XCircle className="w-3.5 h-3.5 flex-shrink-0 text-aurora-error" />
        ) : (
          <Loader2 className={cn('w-3.5 h-3.5 flex-shrink-0 text-aurora-text-muted', isInstalling && 'animate-spin')} />
        )}
        <span className={cn('text-[13px] font-medium', isDone ? 'text-aurora-text-primary' : isFailed ? 'text-aurora-error' : 'text-aurora-text-muted')}>
          {device_id}
        </span>
        <span className={cn('text-[11px] capitalize ml-auto', isDone ? 'text-aurora-success' : isFailed ? 'text-aurora-error' : 'text-aurora-text-muted')}>
          {status}
        </span>
      </div>
      {latest?.file && (
        <div className="text-[11px] text-aurora-text-muted font-mono pl-5 truncate">{latest.file}</div>
      )}
      {latest?.error && (
        <div className="text-[11px] text-aurora-error pl-5 truncate">{latest.error}</div>
      )}
    </div>
  )
}

// ── Main dialog ────────────────────────────────────────────────────────────

type DialogPhase = 'select' | 'progress' | 'done'

export function CherryPickDialog({ pluginId, pluginName, open, onClose, components: propComponents }: CherryPickDialogProps) {
  // Component list state
  const [components, setComponents] = useState<PluginComponent[]>([])
  const [componentsLoading, setComponentsLoading] = useState(false)
  const [componentsError, setComponentsError] = useState<string | null>(null)

  // Selection state
  const [selectedComponents, setSelectedComponents] = useState<Set<string>>(new Set())
  const [selectedDevices, setSelectedDevices] = useState<string[]>([])
  const [scope, setScope] = useState<'global' | 'project'>('global')
  const [projectPath, setProjectPath] = useState('')
  const [pathError, setPathError] = useState<string | null>(null)

  // Install state
  const [phase, setPhase] = useState<DialogPhase>('select')
  const [loading, setLoading] = useState(false)
  const [submitError, setSubmitError] = useState<string | null>(null)
  const [progressByDevice, setProgressByDevice] = useState<Map<string, InstallProgressEvent[]>>(new Map())

  const isSubmittingRef = useRef(false)
  const sseCleanupRef = useRef<(() => void) | null>(null)

  // Load plugin components when dialog opens
  useEffect(() => {
    if (!open) return
    if (propComponents) {
      // Normalize prop-provided components to PluginComponent shape
      const normalized: PluginComponent[] = propComponents.map((c) => ({
        kind: (c.type as PluginComponentKind) ?? 'files',
        name: c.name,
        path: c.path ?? '',
      }))
      setComponents(normalized)
      return
    }
    const controller = new AbortController()
    setComponentsLoading(true)
    setComponentsError(null)
    getPluginComponents(pluginId, controller.signal)
      .then(setComponents)
      .catch((err) => {
        if (isAbortError(err)) return
        setComponentsError(err instanceof Error ? err.message : 'Failed to load components')
      })
      .finally(() => setComponentsLoading(false))
    return () => controller.abort()
  }, [open, pluginId, propComponents])

  // Cleanup SSE on unmount/close
  useEffect(() => {
    return () => {
      sseCleanupRef.current?.()
      sseCleanupRef.current = null
    }
  }, [])

  function handleClose() {
    if (loading) return
    sseCleanupRef.current?.()
    sseCleanupRef.current = null
    // Reset all state
    setSelectedComponents(new Set())
    setSelectedDevices([])
    setScope('global')
    setProjectPath('')
    setPathError(null)
    setPhase('select')
    setLoading(false)
    setSubmitError(null)
    setProgressByDevice(new Map())
    setComponents([])
    setComponentsError(null)
    onClose()
  }

  function validateProjectPath(value: string): string | null {
    if (!value.trim()) return 'Project path is required'
    if (!value.startsWith('/')) return 'Project path must be an absolute path (start with /)'
    return null
  }

  function handleComponentToggle(path: string, checked: boolean) {
    setSelectedComponents((prev) => {
      const next = new Set(prev)
      if (checked) next.add(path)
      else next.delete(path)
      return next
    })
  }

  function handleGroupSelectAll(groupKey: PluginComponentKind, groupPaths: string[], allSelected: boolean) {
    setSelectedComponents((prev) => {
      const next = new Set(prev)
      if (allSelected) {
        groupPaths.forEach((p) => next.delete(p))
      } else {
        groupPaths.forEach((p) => next.add(p))
      }
      return next
    })
  }

  function connectProgressSSE(rpcId: string) {
    const base = normalizeGatewayApiBase()
    const url = `${base}/marketplace/cherry-pick/progress?rpc_id=${encodeURIComponent(rpcId)}`

    const source = new EventSource(url, { withCredentials: true })
    const totalDevices = selectedDevices.length
    const finishedDevices = new Set<string>()

    source.onmessage = (ev) => {
      let event: InstallProgressEvent
      try {
        event = JSON.parse(ev.data) as InstallProgressEvent
      } catch {
        return
      }

      setProgressByDevice((prev) => {
        const next = new Map(prev)
        const existing = next.get(event.device_id) ?? []
        next.set(event.device_id, [...existing, event])
        return next
      })

      if (event.type === 'complete' || event.type === 'error') {
        finishedDevices.add(event.device_id)
        if (finishedDevices.size >= totalDevices) {
          source.close()
          sseCleanupRef.current = null
          setLoading(false)
          setPhase('done')
        }
      }
    }

    source.onerror = () => {
      if (source.readyState === EventSource.CLOSED) {
        setLoading(false)
        setPhase('done')
      }
    }

    sseCleanupRef.current = () => source.close()
  }

  async function handleSubmit() {
    if (isSubmittingRef.current || loading) return
    if (selectedComponents.size === 0 || selectedDevices.length === 0) return

    if (scope === 'project') {
      const err = validateProjectPath(projectPath)
      if (err) {
        setPathError(err)
        return
      }
    }

    isSubmittingRef.current = true
    setLoading(true)
    setSubmitError(null)

    // Initialize pending entries for each device
    const initial = new Map<string, InstallProgressEvent[]>()
    for (const deviceId of selectedDevices) {
      initial.set(deviceId, [])
    }
    setProgressByDevice(initial)
    setPhase('progress')

    try {
      const result = await cherryPickPlugin({
        plugin_id: pluginId,
        components: Array.from(selectedComponents),
        device_ids: selectedDevices,
        scope,
        project_path: scope === 'project' ? projectPath.trim() : undefined,
        confirm: true,
      })

      if (result.rpc_id) {
        connectProgressSSE(result.rpc_id)
      } else {
        // No streaming — treat as immediate success
        setLoading(false)
        setPhase('done')
      }
    } catch (err) {
      setSubmitError(err instanceof Error ? err.message : 'Cherry-pick failed')
      setLoading(false)
      setPhase('select')
    } finally {
      isSubmittingRef.current = false
    }
  }

  // Derived: build grouped components
  const groupedComponents = COMPONENT_GROUPS.map((group) => ({
    ...group,
    items: components.filter((c) => c.kind === group.key),
  })).filter((g) => g.items.length > 0)

  const canSubmit = selectedComponents.size > 0 && selectedDevices.length > 0 && !loading && phase === 'select'
  const deviceProgressEntries = Array.from(progressByDevice.entries())

  return (
    <Dialog open={open} onOpenChange={(v) => { if (!v) handleClose() }}>
      <DialogContent className="w-[600px] max-w-[calc(100vw-40px)] p-0 bg-aurora-panel-strong border-aurora-border-strong rounded-aurora-3 overflow-hidden gap-0 max-h-[calc(100svh-4rem)] flex flex-col">
        <DialogTitle className="sr-only">Cherry-pick components from {pluginName}</DialogTitle>

        {/* Header */}
        <div className="px-7 pt-6 pb-5 border-b border-aurora-border-default bg-[linear-gradient(180deg,color-mix(in_srgb,var(--aurora-panel-strong)_80%,transparent),transparent)] flex-shrink-0">
          <div className="flex items-start gap-3">
            <div className="w-9 h-9 rounded-[11px] flex-shrink-0 flex items-center justify-center text-aurora-accent-strong bg-[linear-gradient(135deg,color-mix(in_srgb,var(--aurora-panel-medium)_88%,transparent),color-mix(in_srgb,var(--aurora-accent-strong)_10%,transparent))] border border-aurora-border-default shadow-[var(--aurora-shadow-small)]">
              <Cherry className="w-[18px] h-[18px]" />
            </div>
            <div className="flex-1 min-w-0">
              <div className="font-display text-[17px] font-extrabold tracking-[-0.02em] text-aurora-text-primary truncate">
                {pluginName}
              </div>
              <p className="text-[12px] text-aurora-text-muted mt-0.5 leading-[1.5]">
                Select components to install
              </p>
            </div>
          </div>
        </div>

        {/* Body — scrollable */}
        <div className="overflow-y-auto flex-1 px-7 py-5 flex flex-col gap-5">
          {/* Destructive warning banner */}
          <div className="flex items-start gap-2.5 rounded-aurora-1 border border-amber-500/30 bg-amber-500/10 px-3 py-2.5">
            <AlertTriangle className="w-4 h-4 flex-shrink-0 text-amber-500 mt-0.5" />
            <p className="text-[12px] text-amber-500 leading-[1.5]">
              This will write files to the selected devices. Existing files at the same paths will be overwritten.
            </p>
          </div>

          {phase === 'select' ? (
            <>
              {/* Component checklist */}
              <div className="flex flex-col gap-3">
                <div className="text-[11px] font-bold uppercase tracking-[0.12em] text-aurora-text-muted">
                  Components
                </div>

                {componentsLoading && (
                  <div className="space-y-2">
                    {[0, 1, 2].map((i) => (
                      <div key={i} className="h-[48px] rounded-aurora-1 border border-aurora-border-strong bg-aurora-control-surface animate-pulse" />
                    ))}
                  </div>
                )}

                {componentsError && (
                  <div className="rounded-aurora-1 border border-aurora-error/30 bg-aurora-error/8 px-3 py-2.5">
                    <p className="text-[12px] text-aurora-error">{componentsError}</p>
                  </div>
                )}

                {!componentsLoading && !componentsError && groupedComponents.length === 0 && (
                  <div className="rounded-aurora-1 border border-aurora-border-strong bg-aurora-control-surface px-3 py-4 text-center">
                    <p className="text-[12px] text-aurora-text-muted">No components found for this plugin</p>
                  </div>
                )}

                {!componentsLoading && groupedComponents.map((group) => {
                  const groupPaths = group.items.map((c) => c.path)
                  const allSelected = groupPaths.every((p) => selectedComponents.has(p))
                  const someSelected = groupPaths.some((p) => selectedComponents.has(p))

                  return (
                    <div key={group.key} className="flex flex-col gap-1.5">
                      {/* Group header with select-all toggle */}
                      <div className="flex items-center justify-between">
                        <span className="text-[11px] font-bold uppercase tracking-[0.12em] text-aurora-text-muted">
                          {group.label}
                        </span>
                        <button
                          type="button"
                          onClick={() => handleGroupSelectAll(group.key, groupPaths, allSelected)}
                          className="text-[11px] text-aurora-accent-primary hover:text-aurora-accent-strong transition-colors duration-150"
                        >
                          {allSelected ? 'Deselect all' : someSelected ? 'Select all' : 'Select all'}
                        </button>
                      </div>
                      <div className="flex flex-col gap-1">
                        {group.items.map((component) => (
                          <ComponentCheckbox
                            key={component.path}
                            id={component.path}
                            label={component.name}
                            path={component.path}
                            checked={selectedComponents.has(component.path)}
                            onChange={handleComponentToggle}
                          />
                        ))}
                      </div>
                    </div>
                  )
                })}

                {selectedComponents.size > 0 && (
                  <p className="text-[11px] text-aurora-text-muted">
                    {selectedComponents.size} component{selectedComponents.size === 1 ? '' : 's'} selected
                  </p>
                )}
              </div>

              {/* Device Selector */}
              <div className="flex flex-col gap-2">
                <div className="text-[11px] font-bold uppercase tracking-[0.12em] text-aurora-text-muted">
                  Target Devices
                </div>
                <DeviceSelector
                  selected={selectedDevices}
                  onChange={setSelectedDevices}
                />
                {selectedDevices.length === 0 && (
                  <p className="text-[11px] text-aurora-text-muted/70">
                    Select at least one device to install on
                  </p>
                )}
              </div>

              {/* Scope Selector */}
              <div className="flex flex-col gap-2">
                <div className="text-[11px] font-bold uppercase tracking-[0.12em] text-aurora-text-muted">
                  Install Scope
                </div>
                <div className="flex flex-col gap-2">
                  <label className="flex items-start gap-3 cursor-pointer">
                    <input
                      type="radio"
                      name="cherry-pick-scope"
                      value="global"
                      checked={scope === 'global'}
                      onChange={() => { setScope('global'); setPathError(null) }}
                      className="mt-0.5 accent-aurora-accent-primary"
                    />
                    <div>
                      <span className="text-[13px] font-medium text-aurora-text-primary">Global</span>
                      <p className="text-[11px] text-aurora-text-muted mt-px">
                        Install to <code className="bg-aurora-control-surface px-1 rounded text-[10px]">~/.claude/</code> — available in all projects
                      </p>
                    </div>
                  </label>
                  <label className="flex items-start gap-3 cursor-pointer">
                    <input
                      type="radio"
                      name="cherry-pick-scope"
                      value="project"
                      checked={scope === 'project'}
                      onChange={() => setScope('project')}
                      className="mt-0.5 accent-aurora-accent-primary"
                    />
                    <div className="flex-1 min-w-0">
                      <span className="text-[13px] font-medium text-aurora-text-primary">Project</span>
                      <p className="text-[11px] text-aurora-text-muted mt-px">
                        Install to a specific project directory
                      </p>
                    </div>
                  </label>
                </div>

                {scope === 'project' && (
                  <div className="flex flex-col gap-1.5 mt-1">
                    <label className="text-[11px] font-bold uppercase tracking-[0.12em] text-aurora-text-muted">
                      Project Path
                    </label>
                    <input
                      type="text"
                      value={projectPath}
                      onChange={(e) => {
                        setProjectPath(e.target.value)
                        if (pathError) setPathError(null)
                      }}
                      placeholder="/home/user/my-project"
                      className={cn(
                        'bg-aurora-control-surface border rounded-aurora-1 text-aurora-text-primary placeholder:text-aurora-text-muted/55 px-[14px] py-[10px] text-[13px] outline-none transition-[border-color,box-shadow] shadow-[var(--aurora-shadow-inset)]',
                        pathError
                          ? 'border-aurora-error focus:border-aurora-error focus:shadow-[0_0_0_3px_color-mix(in_srgb,var(--aurora-error)_20%,transparent)]'
                          : 'border-aurora-border-strong focus:border-aurora-accent-primary focus:shadow-[0_0_0_3px_var(--aurora-focus-ring)]',
                      )}
                    />
                    {pathError && (
                      <p className="text-[11px] text-aurora-error">{pathError}</p>
                    )}
                  </div>
                )}
              </div>
            </>
          ) : (
            /* Progress / Done view */
            <div className="flex flex-col gap-3">
              <div className="text-[11px] font-bold uppercase tracking-[0.12em] text-aurora-text-muted">
                {phase === 'done' ? 'Installation Results' : 'Installing…'}
              </div>
              <div className="flex flex-col bg-aurora-control-surface border border-aurora-border-strong rounded-aurora-1 px-4 shadow-[var(--aurora-shadow-inset)]">
                {deviceProgressEntries.length === 0 ? (
                  <div className="py-4 text-center">
                    <Loader2 className="w-4 h-4 mx-auto animate-spin text-aurora-text-muted" />
                  </div>
                ) : (
                  deviceProgressEntries.map(([deviceId, events]) => (
                    <DeviceProgressRow key={deviceId} device_id={deviceId} events={events} />
                  ))
                )}
              </div>
            </div>
          )}

          {submitError && (
            <div className="rounded-aurora-1 border border-aurora-error/30 bg-aurora-error/8 px-3 py-2.5">
              <p className="text-[12px] text-aurora-error">{submitError}</p>
            </div>
          )}
        </div>

        {/* Footer */}
        <div className="flex justify-end gap-2 px-7 py-4 pb-6 border-t border-aurora-border-default flex-shrink-0">
          <button
            type="button"
            onClick={handleClose}
            disabled={loading}
            className="inline-flex items-center gap-1.5 px-[14px] py-1.5 rounded-lg font-sans text-[13px] font-semibold cursor-pointer border border-transparent bg-transparent text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary transition-all duration-150 disabled:opacity-40 disabled:cursor-not-allowed"
          >
            {phase === 'done' ? 'Close' : 'Cancel'}
          </button>
          {phase === 'select' && (
            <button
              type="button"
              onClick={() => void handleSubmit()}
              disabled={!canSubmit}
              className="inline-flex items-center gap-1.5 px-[14px] py-1.5 rounded-lg font-sans text-[13px] font-semibold cursor-pointer bg-aurora-accent-primary text-aurora-page-bg hover:bg-aurora-accent-strong transition-all duration-150 disabled:opacity-40 disabled:cursor-not-allowed"
            >
              <Cherry className="w-[14px] h-[14px]" />
              {loading
                ? 'Starting…'
                : `Cherry-pick${selectedComponents.size > 0 ? ` ${selectedComponents.size}` : ''} to ${selectedDevices.length > 0 ? selectedDevices.length : ''} ${selectedDevices.length === 1 ? 'device' : 'devices'}`.trim()}
            </button>
          )}
        </div>
      </DialogContent>
    </Dialog>
  )
}
