'use client'

import * as React from 'react'
import {
  ChevronDown,
  ChevronRight,
  CheckCircle2,
  XCircle,
  Loader2,
} from 'lucide-react'
import { cn } from '@/lib/utils'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import type { TranscriptToolCall } from './types'
import { AURORA_MUTED_LABEL } from '@/components/aurora/tokens'
import { getInlineArtifact, getToolPresentation, toDomainChip } from './tool-call-presentation'

interface ToolCallDisplayProps {
  toolCall: TranscriptToolCall
  isChild?: boolean
}

export function ToolCallDisplay({ toolCall, isChild = false }: ToolCallDisplayProps) {
  const [open, setOpen] = React.useState(false)
  const artifact = React.useMemo(() => getInlineArtifact(toolCall), [toolCall])
  const presentation = React.useMemo(() => getToolPresentation(toolCall, artifact), [artifact, toolCall])

  const statusTone =
    toolCall.status === 'failed'
      ? 'text-aurora-error'
      : toolCall.status === 'completed'
        ? 'text-aurora-success'
        : 'text-aurora-accent-primary'

  const statusIcon = toolCall.status === 'failed' ? (
    <XCircle className="size-3.5 text-aurora-error" />
  ) : toolCall.status === 'completed' ? (
    <CheckCircle2 className="size-3.5 text-aurora-success" />
  ) : (
    <Loader2 className="size-3.5 animate-spin text-aurora-accent-primary" />
  )

  return (
    <Collapsible open={open} onOpenChange={setOpen}>
      <CollapsibleTrigger asChild>
        <button
          type="button"
          className={cn(
            'group relative flex w-full items-start gap-3 py-2 text-left',
            !isChild && 'before:absolute before:bottom-0 before:left-[8px] before:top-0 before:w-px before:bg-aurora-border-default/70',
          )}
        >
          <span className="relative z-10 mt-0.5 flex size-4 shrink-0 items-center justify-center rounded-full bg-aurora-page-bg">
            {React.createElement(presentation.icon, {
              className: cn('size-4 shrink-0', statusTone, presentation.accentClassName),
            })}
          </span>
          <div className="min-w-0 flex-1 rounded-aurora-1 px-1 py-0.5 transition-colors group-hover:bg-aurora-hover-bg/30">
            <div className="flex min-w-0 items-start gap-2">
              <div className="min-w-0 flex-1">
                <span className="text-[14px] leading-[1.45] text-aurora-text-primary">
                  {presentation.label}
                </span>
                {toolCall.locations.length > 0 && (
                  <p className="mt-0.5 truncate text-[11px] leading-[1.45] text-aurora-text-muted/70" title={toolCall.locations[0]}>
                    {toolCall.locations[0]}
                  </p>
                )}
              </div>
              <span className="mt-0.5 shrink-0">{statusIcon}</span>
              {open ? (
                <ChevronDown className="mt-0.5 size-3.5 shrink-0 text-aurora-text-muted/50" />
              ) : (
                <ChevronRight className="mt-0.5 size-3.5 shrink-0 text-aurora-text-muted/50" />
              )}
            </div>
            {artifact.commands.length > 0 && (
              <div className="mt-2 space-y-1.5">
                {artifact.commands.slice(0, 2).map((command, index) => (
                  <div
                    key={`${command.cmd ?? command.path ?? 'command'}-${index}`}
                    className={cn(
                      'rounded-aurora-1 border border-aurora-border-default/70 bg-aurora-control-surface px-2.5 py-2',
                      presentation.category === 'command' && 'bg-aurora-success/12',
                      presentation.category === 'read' && 'bg-aurora-accent-primary/12',
                      presentation.category === 'search' && 'bg-aurora-accent-primary/12',
                      presentation.category === 'edit' && 'bg-aurora-warn/12',
                    )}
                  >
                    <p className="text-[12px] font-medium text-aurora-text-primary">
                      {command.type === 'read'
                        ? `Reading ${command.name ?? 'resource'}`
                        : command.type === 'search'
                          ? `Searching ${command.name ?? 'source'}`
                          : command.type === 'write' || command.type === 'edit'
                            ? `Editing ${command.name ?? 'resource'}`
                            : command.name ?? command.path ?? 'Running command'}
                    </p>
                    {command.path ? (
                      <p className="mt-1 truncate text-[11px] leading-[1.45] text-aurora-text-muted">
                        {command.path}
                      </p>
                    ) : null}
                  </div>
                ))}
              </div>
            )}
            {artifact.diffPreview ? (
              <div className="mt-2 overflow-hidden rounded-aurora-2 border border-aurora-warn/18 bg-aurora-warn/12">
                <div className="border-b border-aurora-warn/18 px-2.5 py-2">
                  <p className="text-[12px] font-medium text-aurora-text-primary">
                    {artifact.diffPreview.title}
                  </p>
                </div>
                <div className="space-y-1 px-2.5 py-2 font-mono text-[11px] leading-[1.5]">
                  {artifact.diffPreview.snippet.map((line, index) => (
                    <p
                      key={`${toolCall.id}-diff-${index}`}
                      className={cn(
                        line.startsWith('+') && 'text-aurora-success',
                        line.startsWith('-') && 'text-aurora-error',
                        !line.startsWith('+') && !line.startsWith('-') && 'text-aurora-text-muted',
                      )}
                    >
                      {line}
                    </p>
                  ))}
                </div>
              </div>
            ) : null}
            {artifact.links.length > 0 && (
              <div className="mt-2 flex flex-wrap gap-2">
                {artifact.links.map((url) => (
                  <a
                    key={url}
                    href={url}
                    target="_blank"
                    rel="noreferrer"
                    className="rounded-full border border-aurora-border-default bg-aurora-control-surface px-2.5 py-1 text-[11px] text-aurora-text-primary transition-colors hover:border-aurora-accent-primary/40 hover:text-aurora-accent-primary"
                  >
                    {toDomainChip(url)}
                  </a>
                ))}
              </div>
            )}
            {artifact.imageUrl ? (
              <div className="mt-2 overflow-hidden rounded-aurora-2 border border-aurora-border-default bg-aurora-control-surface">
                {/* eslint-disable-next-line @next/next/no-img-element */}
                <img
                  src={artifact.imageUrl}
                  alt={toolCall.title}
                  className="max-h-48 w-full object-cover"
                />
              </div>
            ) : null}
          </div>
        </button>
      </CollapsibleTrigger>


      <CollapsibleContent>
        <div className="relative ml-2 mt-1 border-l border-aurora-border-default/70 pl-5">
          {artifact.summary ? (
            <p
              className={cn(
                'mb-2 rounded-aurora-1 border border-aurora-border-default/70 px-2.5 py-2 text-[12px] leading-[1.55] text-aurora-text-muted',
                presentation.category === 'review' && 'bg-aurora-warn/12',
                presentation.category === 'media' && 'bg-aurora-accent-deep/12',
                presentation.category === 'source' && 'bg-aurora-success/12',
              )}
            >
              {artifact.summary}
            </p>
          ) : null}
          <div className="overflow-hidden rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface">
            <div className="border-b border-aurora-border-default/60 px-3 py-2">
              <p className={cn(AURORA_MUTED_LABEL, 'mb-1.5 text-aurora-text-muted/60')}>
                Input
              </p>
              <pre className="aurora-scrollbar overflow-x-auto font-mono text-[11px] leading-[1.5] text-aurora-text-primary">
                {JSON.stringify(toolCall.input ?? {}, null, 2)}
              </pre>
            </div>

            {toolCall.output !== undefined && (
              <div className="px-3 py-2">
                <p className={cn(AURORA_MUTED_LABEL, 'mb-1.5 text-aurora-success')}>
                  Output
                </p>
                {typeof toolCall.output === 'string' ? (
                  <div className="aurora-scrollbar max-h-72 overflow-auto rounded-aurora-1 border border-aurora-border-default/70 bg-aurora-page-bg px-3 py-2.5 text-[12px] leading-[1.6] whitespace-pre-wrap text-aurora-text-primary">
                    {toolCall.output}
                  </div>
                ) : (
                  <pre className="aurora-scrollbar overflow-x-auto font-mono text-[11px] leading-[1.5] whitespace-pre-wrap text-aurora-text-primary">
                    {JSON.stringify(toolCall.output, null, 2)}
                  </pre>
                )}
              </div>
            )}
          </div>
        </div>
      </CollapsibleContent>
    </Collapsible>
  )
}
