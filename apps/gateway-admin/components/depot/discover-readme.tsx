'use client'

import { BookOpen } from 'lucide-react'
import { SafeMarkdown } from '@/components/markdown/safe-markdown'

/** Render only server-verified revision content; descriptions are not README substitutes. */
export function DiscoverReadme({ path, content }: { path: string; content: string }) {
  const isReadme = path.split('/').at(-1)?.toLowerCase() === 'readme.md'
  return <section aria-label={isReadme ? 'README' : 'Source document'} className="min-w-0 overflow-hidden rounded-[12px] border border-aurora-border-subtle bg-gradient-to-b from-aurora-panel-strong-top to-aurora-panel-strong">
    <header className="flex flex-wrap items-center justify-between gap-2 border-b border-aurora-border-subtle bg-aurora-control-surface px-[13px] py-[9px]">
      <h3 className="flex items-center gap-2 text-[9.5px] font-bold uppercase tracking-[.13em] text-aurora-text-muted"><BookOpen aria-hidden="true" className="size-[13px] text-aurora-accent-strong" />{isReadme ? 'README' : 'Source document'}</h3>
      <span className="min-w-0 break-all font-mono text-[10px] text-aurora-text-muted">{path}</span>
    </header>
    <div className="px-[14px] pb-[15px] pt-[13px]">
      {content.length ? <SafeMarkdown text={content} className="[&>div]:flex [&>div]:flex-col [&>div]:gap-[11px] [&>div>*]:!my-0 [&_h1]:!leading-[1.3] [&_h2]:!leading-[1.3] [&_h3]:!leading-[1.3] [&_[data-streamdown=code-block]]:!gap-0 [&_[data-streamdown=code-block]]:!rounded-[9px] [&_[data-streamdown=code-block]]:!border-aurora-border-subtle [&_[data-streamdown=code-block]]:!bg-aurora-control-surface [&_[data-streamdown=code-block]]:!p-0 [&_[data-streamdown=code-block-header]]:!h-7 [&_[data-streamdown=code-block-header]]:!px-[10px] [&_[data-streamdown=code-block-body]]:!border-0 [&_[data-streamdown=code-block-body]]:!bg-transparent [&_[data-streamdown=code-block-body]]:!px-[10px] [&_[data-streamdown=code-block-body]]:!py-2 [&_[data-streamdown=code-block-body]]:!text-[11.5px] [&_pre]:!m-0 [&_pre]:!border-0 [&_pre]:!bg-transparent [&_pre]:!p-0 text-[12.5px] leading-[1.65] text-aurora-text-muted [&_h1]:font-display [&_h1]:text-[15px] [&_h1]:font-bold [&_h1]:text-aurora-text-primary [&_h2]:font-display [&_h2]:text-[12.5px] [&_h2]:font-bold [&_h2]:text-aurora-text-primary [&_h3]:font-display [&_h3]:text-[12.5px] [&_h3]:font-bold [&_h3]:text-aurora-text-primary [&_p]:text-pretty [&_pre]:text-[11.5px] [&_ul]:pl-[var(--space-6)] [&_ol]:pl-[var(--space-6)]" /> : <p className="text-[12.5px] leading-[1.65] text-aurora-text-muted">This file is empty.</p>}
    </div>
  </section>
}
