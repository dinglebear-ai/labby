'use client'

import { Clipboard } from 'lucide-react'
import { toast } from 'sonner'
import { composeArtifactSource, artifactLanguage, type ArtifactKind, type ArtifactMetadata } from '@/lib/editor/artifact-standards'

export function ArtifactFrontmatterPreview({ kind, metadata }: { kind: ArtifactKind; metadata: ArtifactMetadata }) {
  if (artifactLanguage(kind) !== 'markdown') return null
  const frontmatter = composeArtifactSource(kind, metadata, '').trimEnd()
  const copy = async () => {
    try { await navigator.clipboard.writeText(frontmatter); toast.success('Frontmatter copied') }
    catch { toast.error('Could not copy frontmatter') }
  }
  return <section aria-label="Generated frontmatter" className="mt-3 overflow-hidden rounded-aurora-2 border border-aurora-border-default bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] shadow-aurora-medium">
    <div className="flex items-center justify-between gap-2 border-b border-aurora-border-default bg-aurora-page-bg/35 px-[15px] py-2.5">
      <h2 className="text-[9.5px] font-bold uppercase tracking-[.13em] text-aurora-text-muted">Frontmatter</h2>
      <button type="button" aria-label="Copy frontmatter" title="Copy frontmatter" onClick={() => void copy()} className="grid size-6 place-items-center rounded-[7px] text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary"><Clipboard className="size-3" /></button>
    </div>
    <pre className="m-0 whitespace-pre-wrap break-words bg-aurora-page-bg/35 px-[15px] py-3 font-mono text-[11px] leading-[1.65] text-aurora-text-primary">{frontmatter}</pre>
    <p className="border-t border-aurora-border-default px-[15px] py-[9px] text-[10.5px] leading-[1.45] text-aurora-text-muted">Generated from the draft fields. Included in the complete Markdown source.</p>
  </section>
}
