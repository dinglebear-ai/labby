'use client'

import * as React from 'react'

export function ArtifactDescriptionField({ value, onChange }: { value: string; onChange: (value: string) => void }) {
  const ref = React.useRef<HTMLTextAreaElement>(null)
  React.useLayoutEffect(() => {
    const field = ref.current
    if (!field) return
    const resize = () => {
      field.style.height = 'auto'
      field.style.height = `${field.scrollHeight + 2}px`
    }
    resize()
    // Reflow when the tips panel or viewport changes the available line width.
    if (typeof ResizeObserver === 'undefined') return
    let width = field.getBoundingClientRect().width
    const observer = new ResizeObserver(entries => {
      const nextWidth = entries[0]?.contentRect.width
      if (nextWidth !== undefined && nextWidth !== width) { width = nextWidth; resize() }
    })
    observer.observe(field)
    return () => observer.disconnect()
  }, [value])
  return <textarea ref={ref} aria-label="Artifact description" name="description" value={value} onChange={event => onChange(event.target.value)} rows={1} placeholder="One line the harness reads to decide when to load this…" className="-mx-1.5 mt-1 block min-h-[52px] w-[calc(100%+12px)] resize-none overflow-hidden rounded-t-lg border border-transparent border-b-aurora-border-strong bg-transparent px-1.5 py-1 text-sm font-[480] leading-[1.5] text-aurora-text-muted outline-none transition-colors [border-bottom-style:dotted] hover:bg-aurora-hover-bg focus:rounded-lg focus:border-aurora-accent-primary focus:bg-aurora-control-surface focus:text-aurora-text-primary focus:shadow-[var(--aurora-focus-ring-strong)] focus:[border-bottom-style:solid]" />
}
