'use client'

import { useState, useEffect, useCallback, useRef } from 'react'
import { 
  Plus, 
  X, 
  GripVertical, 
  Save, 
  AlertTriangle,
  Eye,
  EyeOff,
  Asterisk,
  HelpCircle,
  Loader2,
  CheckCircle2,
} from 'lucide-react'
import { toast } from 'sonner'
import { GatewayApiError } from '@/lib/api/gateway-client'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Badge } from '@/components/ui/badge'
import { Switch } from '@/components/ui/switch'
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui/tooltip'
import { AURORA_DISPLAY_NUMBER } from '@/components/aurora/tokens'
import { cn } from '@/lib/utils'
import { useGatewayMutations, useExposurePolicy, useServiceActions } from '@/lib/hooks/use-gateways'
import type { Gateway, ExposurePolicyPreview } from '@/lib/types/gateway'

interface ExposurePolicyEditorProps {
  gateway: Gateway
}

export function ExposurePolicyEditor({ gateway }: ExposurePolicyEditorProps) {
  const { data: policy, isLoading: policyLoading } = useExposurePolicy(gateway.id)
  const { data: serviceActions } = useServiceActions(gateway.source === 'in_process' ? gateway.id : null)
  const { setExposurePolicy, previewExposurePolicy } = useGatewayMutations()
  const isLabGateway = gateway.source === 'in_process'

  const [mode, setMode] = useState<'expose_all' | 'allowlist'>('expose_all')
  const [patterns, setPatterns] = useState<string[]>([])
  const [newPattern, setNewPattern] = useState('')
  const [preview, setPreview] = useState<ExposurePolicyPreview | null>(null)
  const [previewError, setPreviewError] = useState<string | null>(null)
  const [isPreviewLoading, setIsPreviewLoading] = useState(false)
  const [isSaving, setIsSaving] = useState(false)
  const [hasChanges, setHasChanges] = useState(false)
  const previewRequestId = useRef(0)
  const previousGatewayId = useRef<string | null>(null)

  // Initialize from policy. Preserve local edits for the same gateway, but always
  // reset when the selected gateway changes.
  useEffect(() => {
    if (policy) {
      const gatewayChanged = previousGatewayId.current !== gateway.id
      if (gatewayChanged || !hasChanges) {
        previousGatewayId.current = gateway.id
        setHasChanges(false)
        setPreview(null)
        setPreviewError(null)
        setIsPreviewLoading(false)
        previewRequestId.current += 1
        setMode(policy.mode)
        setPatterns(policy.patterns)
      }
    } else {
      previousGatewayId.current = gateway.id
      setPreview(null)
      setPreviewError(null)
      setIsPreviewLoading(false)
      previewRequestId.current += 1
      setMode('expose_all')
      setPatterns([])
      setHasChanges(false)
    }
  }, [gateway.id, policy, hasChanges])

  // Track changes
  useEffect(() => {
    if (policy) {
      const patternsChanged = JSON.stringify(patterns) !== JSON.stringify(policy.patterns)
      const modeChanged = mode !== policy.mode
      setHasChanges(patternsChanged || modeChanged)
    }
  }, [mode, patterns, policy])

  // Preview debounce — canceled flag prevents stale responses from overwriting fresh ones
  useEffect(() => {
    if (isLabGateway) {
      setPreview(null)
      setPreviewError(null)
      setIsPreviewLoading(false)
      return
    }
    if (mode !== 'allowlist' || patterns.length === 0) {
      setPreview(null)
      setPreviewError(null)
      setIsPreviewLoading(false)
      return
    }

    const requestId = previewRequestId.current + 1
    previewRequestId.current = requestId
    const controller = new AbortController()
    const timer = setTimeout(async () => {
      setIsPreviewLoading(true)
      setPreviewError(null)
      try {
        const result = await previewExposurePolicy(gateway.id, patterns, controller.signal)
        if (previewRequestId.current === requestId) {
          setPreview(result)
          setPreviewError(null)
        }
      } catch (error) {
        if (controller.signal.aborted || (error instanceof DOMException && error.name === 'AbortError')) return
        if (previewRequestId.current === requestId) {
          setPreview(null)
          setPreviewError(
            error instanceof GatewayApiError || error instanceof Error
              ? error.message
              : 'The gateway did not return a usable preview.',
          )
        }
      } finally {
        if (previewRequestId.current === requestId) {
          setIsPreviewLoading(false)
        }
      }
    }, 300)

    return () => {
      controller.abort()
      clearTimeout(timer)
    }
  }, [gateway.id, isLabGateway, mode, patterns, previewExposurePolicy])

  const addPattern = useCallback(() => {
    const trimmed = newPattern.trim()
    if (!trimmed) return
    if (patterns.includes(trimmed)) {
      toast.error('Pattern already exists')
      return
    }
    setPatterns([...patterns, trimmed])
    setNewPattern('')
  }, [newPattern, patterns])

  const removePattern = useCallback((index: number) => {
    setPatterns(patterns.filter((_, i) => i !== index))
  }, [patterns])

  const toggleAction = useCallback((actionName: string, checked: boolean) => {
    setPatterns((current) =>
      checked ? [...current, actionName].sort() : current.filter((pattern) => pattern !== actionName),
    )
  }, [])

  const handleSave = async () => {
    setIsSaving(true)
    try {
      await setExposurePolicy(gateway.id, { mode, patterns })
      toast.success('Exposure policy updated')
      setHasChanges(false)
    } catch (error) {
      toast.error('Failed to update exposure policy', {
        description: error instanceof Error ? error.message : 'The gateway rejected the policy update.',
      })
    } finally {
      setIsSaving(false)
    }
  }

  if (policyLoading) {
    return (
      <div className="rounded-lg border bg-aurora-panel-medium p-6">
        <div className="animate-pulse space-y-4">
          <div className="h-6 w-48 bg-aurora-control-surface rounded" />
          <div className="h-24 bg-aurora-control-surface rounded" />
        </div>
      </div>
    )
  }

  return (
    <div className="space-y-6">
      <div className="rounded-lg border bg-aurora-panel-medium p-6">
        <div className="flex items-center justify-between mb-6">
          <div>
            <h2 className="text-lg font-semibold">Exposure Policy</h2>
            <p className="text-sm text-aurora-text-muted mt-1">
              Control which MCP tools are republished downstream
            </p>
          </div>
          {hasChanges && (
            <Button onClick={handleSave} disabled={isSaving}>
              {isSaving ? (
                <Loader2 className="size-4 mr-2 animate-spin" />
              ) : (
                <Save className="size-4 mr-2" />
              )}
              Save Changes
            </Button>
          )}
        </div>

        {/* Mode Toggle */}
        <div className={cn(
          "flex items-center justify-between rounded-lg border p-4 mb-6 transition-colors",
          mode === 'expose_all' ? 'border-success/30 bg-success/5' : ''
        )}>
          <div className="flex items-center gap-4">
            <div className={cn(
              "flex size-10 items-center justify-center rounded-lg transition-colors",
              mode === 'expose_all' 
                ? 'bg-success/15 text-success' 
                : 'bg-aurora-control-surface text-aurora-text-muted'
            )}>
              <Eye className="size-5" />
            </div>
            <div>
              <Label htmlFor="expose-mode" className="font-medium">Expose All Tools</Label>
              <p className="text-sm text-aurora-text-muted">
                All discovered tools will be available downstream
              </p>
            </div>
          </div>
          <Switch
            id="expose-mode"
            checked={mode === 'expose_all'}
            onCheckedChange={(checked) => setMode(checked ? 'expose_all' : 'allowlist')}
          />
        </div>

        {/* Allowlist Editor */}
        {mode === 'allowlist' && !isLabGateway && (
          <div className="space-y-4">
            <div className="flex items-center gap-2">
              <EyeOff className="size-4 text-aurora-text-muted" />
              <Label className="font-medium">Allowlist Patterns</Label>
              <TooltipProvider>
                <Tooltip>
                  <TooltipTrigger asChild>
                    <HelpCircle className="size-4 text-aurora-text-muted cursor-help" />
                  </TooltipTrigger>
                  <TooltipContent side="right" className="max-w-xs">
                    <p className="text-sm">
                      Use exact tool names or wildcard patterns:
                    </p>
                    <ul className="text-sm mt-2 space-y-1">
                      <li><code className="bg-aurora-control-surface px-1 rounded">read_file</code> - exact match</li>
                      <li><code className="bg-aurora-control-surface px-1 rounded">search_*</code> - prefix wildcard</li>
                      <li><code className="bg-aurora-control-surface px-1 rounded">*</code> - match all</li>
                    </ul>
                  </TooltipContent>
                </Tooltip>
              </TooltipProvider>
            </div>

            {/* Pattern input */}
            <div className="flex gap-2">
              <Input
                placeholder="Enter tool name or pattern (e.g., read_*, search_messages)"
                value={newPattern}
                onChange={(e) => setNewPattern(e.target.value)}
                onKeyDown={(e) => e.key === 'Enter' && addPattern()}
                className="font-mono"
              />
              <Button onClick={addPattern} disabled={!newPattern.trim()}>
                <Plus className="size-4 mr-2" />
                Add
              </Button>
            </div>

            {/* Pattern list */}
            {patterns.length === 0 ? (
              <div className="rounded-lg border border-dashed p-8 text-center">
                <EyeOff className="size-8 mx-auto text-aurora-text-muted mb-3 opacity-50" />
                <p className="text-aurora-text-muted">No patterns configured</p>
                <p className="text-sm text-aurora-text-muted mt-1">
                  Add patterns to specify which tools should be exposed
                </p>
              </div>
            ) : (
              <div className="space-y-2">
                {patterns.map((pattern, index) => (
                  <div
                    key={index}
                    className="flex items-center gap-2 rounded-lg border bg-aurora-control-surface/30 px-3 py-2 group"
                  >
                    <GripVertical className="size-4 text-aurora-text-muted opacity-50" />
                    <div className="flex items-center gap-2 flex-1">
                      {pattern.includes('*') && (
                        <Asterisk className="size-3.5 text-aurora-text-muted" />
                      )}
                      <code className="text-sm font-mono">{pattern}</code>
                    </div>
                    {preview && (
                      <PreviewBadge pattern={pattern} preview={preview} />
                    )}
                    <Button
                      variant="ghost"
                      size="icon"
                      className="size-7 opacity-0 group-hover:opacity-100 transition-opacity"
                      onClick={() => removePattern(index)}
                    >
                      <X className="size-4" />
                    </Button>
                  </div>
                ))}
              </div>
            )}
          </div>
        )}
      </div>

      {/* Preview Panel */}
      {isLabGateway && mode === 'allowlist' && (
        <div className="rounded-lg border bg-aurora-panel-medium p-6">
          <div className="flex items-center justify-between mb-4">
            <h3 className="font-semibold">Available Actions</h3>
            <Badge variant="secondary">
              {(serviceActions ?? []).filter((action) => !['help', 'schema'].includes(action.name)).length}
            </Badge>
          </div>
          <div className="space-y-3">
            {(serviceActions ?? [])
              .filter((action) => !['help', 'schema'].includes(action.name))
              .map((action) => {
                const checked = patterns.includes(action.name)
                return (
                  <label
                    key={action.name}
                    className="flex items-start gap-3 rounded-lg border p-3 cursor-pointer hover:bg-aurora-control-surface/30"
                  >
                    <Checkbox
                      checked={checked}
                      onCheckedChange={(value) => toggleAction(action.name, value === true)}
                    />
                    <div className="space-y-1">
                      <div className="flex items-center gap-2">
                        <code className="text-sm font-mono">{action.name}</code>
                        {action.destructive && (
                          <Badge variant="outline" className="text-[10px] uppercase">
                            Destructive
                          </Badge>
                        )}
                      </div>
                      <p className="text-sm text-aurora-text-muted">{action.description}</p>
                    </div>
                  </label>
                )
              })}
          </div>
        </div>
      )}

      {!isLabGateway && mode === 'allowlist' && patterns.length > 0 && (
        <div className="rounded-lg border bg-aurora-panel-medium p-6">
          <div className="flex items-center justify-between mb-4">
            <h3 className="font-semibold">Preview</h3>
            {isPreviewLoading && (
              <Loader2 className="size-4 animate-spin text-aurora-text-muted" />
            )}
          </div>

          {previewError ? (
            <div role="alert" className="rounded-lg border border-destructive/35 bg-destructive/8 p-4">
              <div className="flex items-center gap-2 text-destructive">
                <AlertTriangle className="size-4" />
                <span className="text-sm font-semibold">Preview unavailable</span>
              </div>
              <p className="mt-2 text-sm text-aurora-text-muted">
                {previewError} The saved exposure policy has not been changed. Fix the gateway connection or discovery error and retry the preview before saving.
              </p>
            </div>
          ) : preview ? (
            <div className="space-y-4">
              {/* Stats */}
              <div className="grid gap-4 sm:grid-cols-3">
                <div className="flex items-center gap-3 rounded-lg border-2 border-aurora-preview-allowed/50 bg-aurora-preview-allowed/15 p-3 shadow-md shadow-aurora-preview-allowed/10">
                  <CheckCircle2 className="size-5 text-aurora-preview-allowed" />
                  <div>
                    <p className={cn(AURORA_DISPLAY_NUMBER, 'text-aurora-preview-allowed')}>{preview.exposed_count}</p>
                    <p className="text-xs text-aurora-text-muted">Exposed</p>
                  </div>
                </div>
                <div className="flex items-center gap-3 rounded-lg border p-3">
                  <EyeOff className="size-5 text-aurora-text-muted" />
                  <div>
                    <p className={cn(AURORA_DISPLAY_NUMBER, 'text-aurora-text-primary')}>{preview.filtered_count}</p>
                    <p className="text-xs text-aurora-text-muted">Filtered</p>
                  </div>
                </div>
                {preview.unmatched_patterns.length > 0 && (
                  <div className="flex items-center gap-3 rounded-lg border-2 border-aurora-preview-unmatched/50 bg-aurora-preview-unmatched/15 p-3 shadow-md shadow-aurora-preview-unmatched/10">
                    <AlertTriangle className="size-5 text-aurora-preview-unmatched" />
                    <div>
                      <p className={cn(AURORA_DISPLAY_NUMBER, 'text-aurora-preview-unmatched')}>{preview.unmatched_patterns.length}</p>
                      <p className="text-xs text-aurora-text-muted">Unmatched</p>
                    </div>
                  </div>
                )}
              </div>

              {/* Unmatched warnings */}
              {preview.unmatched_patterns.length > 0 && (
                <div className="rounded-lg border-2 border-aurora-preview-unmatched/50 bg-gradient-to-r from-aurora-preview-unmatched/15 to-aurora-preview-highlight/10 p-4 shadow-md shadow-aurora-preview-unmatched/10">
                  <div className="flex items-center gap-2 text-aurora-preview-unmatched mb-2">
                    <AlertTriangle className="size-4" />
                    <span className="text-sm font-semibold">Unmatched Patterns</span>
                  </div>
                  <p className="text-sm text-aurora-text-muted mb-3">
                    These patterns don&apos;t match any discovered tools:
                  </p>
                  <div className="flex flex-wrap gap-2">
                    {preview.unmatched_patterns.map((pattern) => (
                      <Badge key={pattern} variant="outline" className="font-mono text-xs border-aurora-preview-unmatched/50 text-aurora-preview-unmatched bg-aurora-preview-unmatched/10">
                        {pattern}
                      </Badge>
                    ))}
                  </div>
                </div>
              )}

              {/* Matched tools */}
              {preview.matched_tools.length > 0 && (
                <div>
                  <h4 className="text-sm font-semibold mb-2 flex items-center gap-2 text-aurora-preview-allowed">
                    <Eye className="size-4" />
                    Exposed Tools
                  </h4>
                  <div className="flex flex-wrap gap-2">
                    {preview.matched_tools.slice(0, 10).map((tool) => (
                      <Badge key={tool.name} variant="secondary" className="font-mono text-xs bg-aurora-preview-allowed/20 text-aurora-preview-allowed border-aurora-preview-allowed/50 border shadow-sm shadow-aurora-preview-allowed/20">
                        {tool.name}
                      </Badge>
                    ))}
                    {preview.matched_tools.length > 10 && (
                      <Badge variant="outline" className="border-info/40 bg-info/10 text-xs text-info">
                        +{preview.matched_tools.length - 10} more
                      </Badge>
                    )}
                  </div>
                </div>
              )}

              {/* Filtered tools */}
              {preview.filtered_tools.length > 0 && (
                <div>
                  <h4 className="text-sm font-medium mb-2 flex items-center gap-2 text-aurora-text-muted">
                    <EyeOff className="size-4" />
                    Filtered Tools
                  </h4>
                  <div className="flex flex-wrap gap-2">
                    {preview.filtered_tools.slice(0, 10).map((tool) => (
                      <Badge key={tool} variant="outline" className="font-mono text-xs opacity-60">
                        {tool}
                      </Badge>
                    ))}
                    {preview.filtered_tools.length > 10 && (
                      <Badge variant="outline" className="text-xs opacity-60">
                        +{preview.filtered_tools.length - 10} more
                      </Badge>
                    )}
                  </div>
                </div>
              )}
            </div>
          ) : (
            <div className="text-center py-8 text-aurora-text-muted">
              <p>Preview will appear when patterns are configured</p>
            </div>
          )}
        </div>
      )}
    </div>
  )
}

function PreviewBadge({ 
  pattern, 
  preview 
}: { 
  pattern: string
  preview: ExposurePolicyPreview 
}) {
  const isUnmatched = preview.unmatched_patterns.includes(pattern)
  const matchCount = preview.matched_tools.filter(t => t.matched_by === pattern).length

  if (isUnmatched) {
    return (
      <Badge variant="outline" className="text-xs border-warning/50 text-warning-foreground">
        No matches
      </Badge>
    )
  }

  if (matchCount > 0) {
    return (
      <Badge variant="secondary" className="text-xs bg-success/15 text-success border border-success/30">
        {matchCount} tool{matchCount !== 1 ? 's' : ''}
      </Badge>
    )
  }

  return null
}
