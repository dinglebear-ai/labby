'use client'

import { useEffect, useMemo, useState } from 'react'
import { Loader2 } from 'lucide-react'
import { toast } from 'sonner'

import { Button } from '@/components/ui/button'
import { SettingsCard, SettingsRow, SettingsToggle } from '@/components/settings/SettingsChrome'
import { useGatewayCodeModeConfig, useGatewayMutations } from '@/lib/hooks/use-gateways'
import type {
  CodeModeSearchConfig,
  CodeModeSearchKind,
  CodeModeSearchSource,
} from '@/lib/types/gateway'
import { getErrorMessage } from '@/lib/utils'

const SOURCE_OPTIONS: ReadonlyArray<{
  value: CodeModeSearchSource
  label: string
  description: string
}> = [
  {
    value: 'public_depot',
    label: 'Public Depot',
    description: 'Search the public Depot catalog, including its full skill index.',
  },
  {
    value: 'team_depot',
    label: 'Team Depot',
    description: 'Search artifacts shared through this Labby instance’s team Depot connection.',
  },
  {
    value: 'personal_labby',
    label: 'Personal Labby',
    description: 'Search tools and artifacts available directly from this Labby instance.',
  },
]

const KIND_OPTIONS: ReadonlyArray<{ value: CodeModeSearchKind; label: string }> = [
  { value: 'tool', label: 'Tools' },
  { value: 'skill', label: 'Skills' },
  { value: 'command', label: 'Commands' },
  { value: 'prompt', label: 'Prompts' },
  { value: 'subagent', label: 'Subagents' },
  { value: 'snippet', label: 'Snippets' },
]

function normalizeSearch(search: CodeModeSearchConfig): CodeModeSearchConfig {
  return {
    sources: SOURCE_OPTIONS.map(({ value }) => value).filter((value) => search.sources.includes(value)),
    kinds: KIND_OPTIONS.map(({ value }) => value).filter((value) => search.kinds.includes(value)),
  }
}

function sameSearch(left: CodeModeSearchConfig, right: CodeModeSearchConfig): boolean {
  const a = normalizeSearch(left)
  const b = normalizeSearch(right)
  return a.sources.join('\0') === b.sources.join('\0') && a.kinds.join('\0') === b.kinds.join('\0')
}

function toggleValue<T extends string>(values: T[], value: T, enabled: boolean): T[] {
  return enabled ? [...new Set([...values, value])] : values.filter((entry) => entry !== value)
}

export function CodeModeSearchSettings(): React.ReactElement {
  const { data: config, isLoading, error, mutate } = useGatewayCodeModeConfig()
  const { setCodeModeConfig } = useGatewayMutations()
  const [baseline, setBaseline] = useState<CodeModeSearchConfig | null>(null)
  const [draft, setDraft] = useState<CodeModeSearchConfig | null>(null)
  const [saving, setSaving] = useState(false)
  const [saveError, setSaveError] = useState<string | null>(null)
  const [serverChanged, setServerChanged] = useState(false)
  const dirty = Boolean(baseline && draft && !sameSearch(baseline, draft))

  useEffect(() => {
    if (!config) return
    const next = normalizeSearch(config.search)
    if (!baseline || (!dirty && !sameSearch(baseline, next))) {
      setBaseline(next)
      setDraft(next)
      setServerChanged(false)
    } else if (baseline && !sameSearch(baseline, next)) {
      setServerChanged(true)
    }
  }, [baseline, config, dirty])

  useEffect(() => {
    if (!dirty) return
    const warnBeforeUnload = (event: BeforeUnloadEvent) => event.preventDefault()
    window.addEventListener('beforeunload', warnBeforeUnload)
    return () => window.removeEventListener('beforeunload', warnBeforeUnload)
  }, [dirty])

  const selectedKindCount = draft?.kinds.length ?? 0
  const status = useMemo(() => {
    if (saving) return 'Saving search settings…'
    if (dirty) return 'Unsaved changes'
    return 'Saved settings apply to new searches immediately.'
  }, [dirty, saving])

  function resetToServer() {
    if (!config) return
    const next = normalizeSearch(config.search)
    setBaseline(next)
    setDraft(next)
    setSaveError(null)
    setServerChanged(false)
  }

  async function save() {
    if (!draft || saving) return
    setSaving(true)
    setSaveError(null)
    try {
      const saved = await setCodeModeConfig({
        search_sources: draft.sources,
        search_kinds: draft.kinds,
      })
      const next = normalizeSearch(saved.search)
      setBaseline(next)
      setDraft(next)
      setServerChanged(false)
      toast.success('Code Mode search settings saved.')
    } catch (cause) {
      setSaveError(getErrorMessage(cause, 'Failed to save Code Mode search settings'))
      await mutate()
    } finally {
      setSaving(false)
    }
  }

  return (
    <SettingsCard
      title="Code Mode search"
      description="Choose which catalogs and artifact families Code Mode includes when it searches. Empty selections intentionally disable that part of discovery."
      action={
        <div className="flex items-center gap-2">
          <span aria-live="polite" className="text-[11px] text-aurora-text-muted">{status}</span>
          <Button type="button" size="sm" variant="outline" disabled={!dirty || saving} onClick={resetToServer}>
            Reset
          </Button>
          <Button type="button" size="sm" disabled={!dirty || saving} onClick={() => void save()}>
            {saving ? <Loader2 aria-hidden="true" className="size-4 animate-spin" /> : null}
            Save
          </Button>
        </div>
      }
    >
      {error ? (
        <div role="alert" className="px-4 py-3 text-sm text-aurora-error">
          Code Mode search settings are unavailable. {getErrorMessage(error, 'Request failed')}
        </div>
      ) : null}
      {saveError ? <div role="alert" className="px-4 py-3 text-sm text-aurora-error">{saveError}</div> : null}
      {serverChanged ? (
        <div role="status" className="px-4 py-3 text-sm text-aurora-warn">
          Server settings changed while you were editing. Your unsaved choices were preserved; reset to load server truth.
        </div>
      ) : null}
      {isLoading && !draft ? (
        <div className="flex items-center gap-2 px-4 py-3 text-sm text-aurora-text-muted">
          <Loader2 aria-hidden="true" className="size-4 animate-spin" /> Loading search settings…
        </div>
      ) : null}
      {draft ? (
        <>
          <fieldset disabled={saving}>
            <legend className="sr-only">Code Mode search sources</legend>
            {SOURCE_OPTIONS.map((option) => (
              <SettingsRow
                key={option.value}
                label={option.label}
                description={option.description}
                control={
                  <SettingsToggle
                    checked={draft.sources.includes(option.value)}
                    disabled={saving}
                    label={`Search ${option.label}`}
                    onChange={(checked) => setDraft((current) => current && ({
                      ...current,
                      sources: toggleValue(current.sources, option.value, checked),
                    }))}
                  />
                }
              />
            ))}
          </fieldset>
          <SettingsRow
            layout="stacked"
            label="Artifact families"
            description={`${selectedKindCount} of ${KIND_OPTIONS.length} families included.`}
            control={
              <fieldset disabled={saving} className="grid grid-cols-2 gap-x-5 gap-y-3 sm:grid-cols-3">
                <legend className="sr-only">Code Mode artifact families</legend>
                {KIND_OPTIONS.map((option) => (
                  <label key={option.value} className="flex items-center justify-between gap-3 text-[12px] font-medium text-aurora-text-primary">
                    {option.label}
                    <SettingsToggle
                      checked={draft.kinds.includes(option.value)}
                      disabled={saving}
                      label={`Search ${option.label}`}
                      onChange={(checked) => setDraft((current) => current && ({
                        ...current,
                        kinds: toggleValue(current.kinds, option.value, checked),
                      }))}
                    />
                  </label>
                ))}
              </fieldset>
            }
          />
        </>
      ) : null}
    </SettingsCard>
  )
}
