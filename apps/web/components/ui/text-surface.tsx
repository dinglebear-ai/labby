'use client'

import React from 'react'
import { EditorState, type Extension } from '@codemirror/state'
import { EditorView } from '@codemirror/view'

import { collectEditorAutocomplete, collectEditorDiagnostics } from '@/lib/editor/diagnostics-registry'
import type { EditorDiagnostic, EditorLanguage } from '@/lib/editor/types'
import { loadLanguageExtension } from '@/lib/editor/language-registry'
import { cn } from '@/lib/utils'
import {
  baseTextSurfaceExtensions,
  diagnosticsCompartment,
  diagnosticsExtension,
  editableCompartment,
  languageCompartment,
} from './text-surface-theme'
import { TextSurfaceToolbar } from './text-surface-toolbar'

export interface TextSurfaceProps {
  path: string
  value: string
  mode: 'view' | 'edit'
  language: EditorLanguage
  dirty?: boolean
  diagnostics?: EditorDiagnostic[]
  onChange?: (next: string) => void
  onSave?: () => void
  onDeploy?: () => void
  onCopy?: () => void
  embedded?: boolean
  showToolbar?: boolean
}

function createState(doc: string, editable: boolean, diagnostics: EditorDiagnostic[]): EditorState {
  return EditorState.create({
    doc,
    extensions: [
      ...baseTextSurfaceExtensions({ editable, diagnostics }),
      EditorView.contentAttributes.of({ 'aria-label': 'Artifact source editor' }),
    ],
  })
}

export function TextSurface({ path, value, mode, language, dirty = false, diagnostics, onChange, onSave, onDeploy, onCopy, embedded = false, showToolbar = true }: TextSurfaceProps) {
  const hostRef = React.useRef<HTMLDivElement | null>(null)
  const viewRef = React.useRef<EditorView | null>(null)
  const onChangeRef = React.useRef(onChange)
  const initialValueRef = React.useRef(value)
  const initialEditableRef = React.useRef(mode === 'edit')
  const initialDiagnosticsRef = React.useRef(diagnostics ?? [])
  const [resolvedDiagnostics, setResolvedDiagnostics] = React.useState<EditorDiagnostic[]>(diagnostics ?? [])
  const [enhancementWarnings, setEnhancementWarnings] = React.useState<Record<string, string>>({})

  const setEnhancementWarning = React.useCallback((key: string, message?: string) => {
    setEnhancementWarnings((current) => {
      if (message) {
        if (current[key] === message) return current
        return { ...current, [key]: message }
      }
      if (!(key in current)) return current
      const next = { ...current }
      delete next[key]
      return next
    })
  }, [])

  React.useEffect(() => {
    onChangeRef.current = onChange
  }, [onChange])

  React.useEffect(() => {
    let cancelled = false
    if (diagnostics) {
      setResolvedDiagnostics(diagnostics)
      setEnhancementWarning('diagnostics')
      return
    }
    collectEditorDiagnostics(path, value)
      .then((next) => {
        if (!cancelled) {
          setResolvedDiagnostics(next)
          setEnhancementWarning('diagnostics')
        }
      })
      .catch((error) => {
        if (!cancelled) {
          setEnhancementWarning(
            'diagnostics',
            `Diagnostics unavailable: ${error instanceof Error ? error.message : 'request failed'}`,
          )
        }
      })
    return () => {
      cancelled = true
    }
  }, [diagnostics, path, setEnhancementWarning, value])

  React.useEffect(() => {
    if (!hostRef.current || viewRef.current) return

    const view = new EditorView({
      state: createState(initialValueRef.current, initialEditableRef.current, initialDiagnosticsRef.current),
      parent: hostRef.current,
      dispatch(transaction) {
        view.update([transaction])
        if (transaction.docChanged) {
          onChangeRef.current?.(transaction.state.doc.toString())
        }
      },
    })
    viewRef.current = view

    return () => {
      view.destroy()
      viewRef.current = null
    }
  }, [])

  React.useEffect(() => {
    const view = viewRef.current
    if (!view) return
    const current = view.state.doc.toString()
    if (current !== value) {
      view.dispatch({ changes: { from: 0, to: current.length, insert: value } })
    }
  }, [value])

  React.useEffect(() => {
    const view = viewRef.current
    if (!view) return
    view.dispatch({
      effects: [
        editableCompartment.reconfigure(EditorView.editable.of(mode === 'edit')),
        diagnosticsCompartment.reconfigure(diagnosticsExtension(resolvedDiagnostics)),
      ],
    })
  }, [mode, resolvedDiagnostics])

  React.useEffect(() => {
    let cancelled = false
    void Promise.allSettled([loadLanguageExtension(language), collectEditorAutocomplete(path, value)]).then(([languageResult, autocompleteResult]) => {
      const view = viewRef.current
      if (!view || cancelled) return
      if (languageResult.status === 'fulfilled') {
        view.dispatch({ effects: [languageCompartment.reconfigure(languageResult.value as Extension)] })
        setEnhancementWarning('language')
      } else {
        setEnhancementWarning(
          'language',
          `Syntax support unavailable: ${languageResult.reason instanceof Error ? languageResult.reason.message : 'load failed'}`,
        )
      }
      if (autocompleteResult.status === 'fulfilled') {
        view.dom.dataset.autocompleteCount = String(autocompleteResult.value.length)
        setEnhancementWarning('autocomplete')
      } else {
        delete view.dom.dataset.autocompleteCount
        setEnhancementWarning(
          'autocomplete',
          `Autocomplete unavailable: ${autocompleteResult.reason instanceof Error ? autocompleteResult.reason.message : 'request failed'}`,
        )
      }
    })
    return () => {
      cancelled = true
    }
  }, [language, path, setEnhancementWarning, value])

  return (
    <div className={cn(
      'aurora-text-surface flex h-full min-h-0 flex-col overflow-hidden bg-aurora-panel-strong',
      !embedded && 'rounded-aurora-2 border border-aurora-border-strong shadow-[var(--aurora-shadow-strong),var(--aurora-highlight-strong)]',
    )}>
      {showToolbar ? <TextSurfaceToolbar
        path={path}
        language={language}
        dirty={dirty}
        diagnostics={resolvedDiagnostics}
        canEdit={mode === 'edit'}
        onSave={onSave}
        onDeploy={onDeploy}
        onCopy={onCopy}
      /> : null}
      {Object.keys(enhancementWarnings).length > 0 ? (
        <div role="status" className="border-b border-aurora-warn/30 bg-aurora-warn/8 px-3 py-1.5 text-[11px] text-aurora-warn">
          Editor partially degraded: {Object.values(enhancementWarnings).join(' · ')}
        </div>
      ) : null}
      <div className="min-h-0 flex-1 overflow-hidden bg-aurora-page-bg">
        <div ref={hostRef} className="cm-editor h-full" aria-label="Code editor" />
      </div>
    </div>
  )
}
