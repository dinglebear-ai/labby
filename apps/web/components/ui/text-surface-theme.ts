import { Compartment, type Extension } from '@codemirror/state'
import { EditorView, drawSelection, highlightActiveLine, lineNumbers, keymap } from '@codemirror/view'
import { HighlightStyle, syntaxHighlighting, foldGutter } from '@codemirror/language'
import { tags } from '@lezer/highlight'
import { defaultKeymap, history, historyKeymap } from '@codemirror/commands'
import { search, searchKeymap } from '@codemirror/search'
import { autocompletion, closeBrackets, completionKeymap } from '@codemirror/autocomplete'
import { lintGutter, linter } from '@codemirror/lint'

import type { EditorDiagnostic } from '@/lib/editor/types'

export const languageCompartment = new Compartment()
export const editableCompartment = new Compartment()
export const diagnosticsCompartment = new Compartment()

const auroraHighlightStyle = HighlightStyle.define([
  { tag: tags.heading, color: '#7dd3c7', fontWeight: '700' },
  { tag: tags.strong, color: '#c78490', fontWeight: '700' },
  { tag: tags.emphasis, color: '#c6a36b', fontStyle: 'italic' },
  { tag: [tags.monospace, tags.literal], color: '#c78490' },
  { tag: tags.quote, color: '#c6a36b', fontStyle: 'italic' },
  { tag: [tags.keyword, tags.bool, tags.atom, tags.typeName], color: '#c78490' },
  { tag: [tags.string, tags.inserted], color: '#7dd3c7' },
  { tag: [tags.link, tags.url], color: '#7dd3c7', textDecoration: 'underline' },
  { tag: [tags.number, tags.integer, tags.float], color: '#c6a36b' },
  { tag: [tags.comment, tags.meta, tags.processingInstruction], color: '#91a9b8' },
  { tag: [tags.punctuation, tags.separator, tags.list], color: '#a8bdc9' },
  { tag: [tags.function(tags.variableName), tags.labelName], color: '#7dd3c7' },
  { tag: [tags.invalid], color: '#c78490', textDecoration: 'underline wavy' },
])

function severityClass(severity: EditorDiagnostic['severity']): 'error' | 'warning' | 'info' {
  return severity === 'error' ? 'error' : severity === 'warning' ? 'warning' : 'info'
}

export function auroraTextSurfaceTheme(): Extension {
  return [
    EditorView.theme({
      '&': {
        color: 'var(--aurora-text-primary)',
        backgroundColor: 'transparent',
        fontSize: '13px',
      },
      '.cm-content': {
        fontFamily: 'var(--font-geist-mono, ui-monospace, SFMono-Regular, monospace)',
        caretColor: 'var(--aurora-text-primary)',
      },
      '.cm-cursor, .cm-dropCursor': { borderLeftColor: 'var(--aurora-text-primary)' },
      '&.cm-focused': { outline: 'none' },
      '.cm-activeLine, .cm-activeLineGutter': { backgroundColor: 'color-mix(in srgb, var(--aurora-accent-primary) 8%, transparent)' },
      '.cm-selectionBackground, ::selection': { backgroundColor: 'color-mix(in srgb, var(--aurora-accent-primary) 22%, transparent)' },
      '.cm-gutters': {
        backgroundColor: 'transparent',
        color: 'var(--aurora-text-muted)',
        borderRight: '1px solid var(--aurora-border-default)',
      },
      '.cm-activeLineGutter': { backgroundColor: 'transparent' },
      '.cm-panels, .cm-tooltip': {
        backgroundColor: 'var(--aurora-panel-strong)',
        color: 'var(--aurora-text-primary)',
        border: '1px solid var(--aurora-border-strong)',
      },
    }),
    syntaxHighlighting(auroraHighlightStyle, { fallback: true }),
  ]
}

export function diagnosticsExtension(diagnostics: EditorDiagnostic[]): Extension {
  return linter(() => diagnostics.map((item) => ({
    from: item.from,
    to: item.to,
    severity: severityClass(item.severity),
    message: item.message,
  })))
}

export function baseTextSurfaceExtensions({ editable, diagnostics }: { editable: boolean; diagnostics: EditorDiagnostic[] }): Extension[] {
  return [
    lineNumbers(),
    highlightActiveLine(),
    drawSelection(),
    history(),
    foldGutter(),
    search({ top: true }),
    autocompletion(),
    closeBrackets(),
    lintGutter(),
    keymap.of([...defaultKeymap, ...historyKeymap, ...searchKeymap, ...completionKeymap]),
    languageCompartment.of([]),
    editableCompartment.of(EditorView.editable.of(editable)),
    diagnosticsCompartment.of(diagnosticsExtension(diagnostics)),
    auroraTextSurfaceTheme(),
  ]
}
