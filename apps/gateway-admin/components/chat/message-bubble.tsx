'use client'

import * as React from 'react'
import { Bot, Check, ChevronDown, Copy, ListChecks, UserRound } from 'lucide-react'
import {
  Streamdown,
  defaultUrlTransform,
  type AllowElement,
  type UrlTransform,
} from 'streamdown'
import { cn } from '@/lib/utils'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import {
  ChainOfThought,
  ChainOfThoughtContent,
  ChainOfThoughtHeader,
} from '@/components/ai/chain-of-thought'
import { Reasoning, ReasoningContent, ReasoningTrigger } from '@/components/ai/reasoning'
import { ToolCallDisplay } from './tool-call-display'
import { GroupedToolCallDisplay, groupConsecutiveToolCalls } from './grouped-tool-call-display'
import { getToolCategory } from './tool-call-presentation'
import type { ACPMessage } from './types'

function CopyButton({ text }: { text: string }) {
  const [copied, setCopied] = React.useState(false)

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(text)
      setCopied(true)
      window.setTimeout(() => setCopied(false), 2000)
    } catch {
      setCopied(false)
    }
  }

  return (
    <Button
      variant="ghost"
      size="icon"
      onClick={handleCopy}
      aria-label="Copy message"
      className="size-6 shrink-0 rounded text-aurora-text-muted/40 transition-opacity hover:bg-aurora-hover-bg hover:text-aurora-text-muted sm:opacity-0 sm:group-hover/bubble:opacity-100 sm:focus-visible:opacity-100"
    >
      {copied ? <Check className="size-3 text-aurora-success" /> : <Copy className="size-3" />}
    </Button>
  )
}

export function getMessageCopyText(message: Pick<ACPMessage, 'text'>) {
  return message.text
}

function StreamingCursor() {
  return (
    <span
      aria-hidden="true"
      className="ml-0.5 inline-block h-3.5 w-0.5 animate-pulse rounded-sm bg-aurora-accent-primary align-middle"
    />
  )
}

const SAFE_MARKDOWN_IMAGE_ELEMENTS = ['img'] as const
const NO_REHYPE_PLUGINS: never[] = []
const DISABLED_LINK_SAFETY = { enabled: false } as const
const MESSAGE_CONTENT_CLASS =
  'relative max-w-full overflow-hidden rounded-aurora-2 px-4 py-3'
const ASSISTANT_MESSAGE_CONTENT_CLASS =
  'border border-aurora-border-default bg-aurora-panel-medium shadow-[var(--aurora-shadow-medium),var(--aurora-highlight-medium)]'
const USER_MESSAGE_CONTENT_CLASS =
  'border border-aurora-border-strong bg-aurora-panel-strong shadow-[var(--aurora-shadow-medium),var(--aurora-highlight-medium)]'

function isAllowedMarkdownUrl(url: string) {
  const trimmed = url.trim()
  if (trimmed.startsWith('//')) return false

  const scheme = trimmed.match(/^([a-z][a-z0-9+.-]*):/i)?.[1]?.toLowerCase()
  return !scheme || scheme === 'http' || scheme === 'https' || scheme === 'mailto'
}

const safeMarkdownUrlTransform: UrlTransform = (url, key, node) => {
  const transformed = defaultUrlTransform(url, key, node)
  if (!transformed) return transformed

  return isAllowedMarkdownUrl(transformed) ? transformed : null
}

const allowSafeMarkdownElement: AllowElement = (element) => {
  if (element.tagName === 'img') return false

  if (element.tagName === 'a') {
    const href = element.properties?.href
    return typeof href === 'string' && isAllowedMarkdownUrl(href)
  }

  return true
}

function AssistantMarkdown({
  text,
  isStreaming,
}: {
  text: string
  isStreaming: boolean
}) {
  return (
    <div className="min-w-0 max-w-full overflow-hidden pr-8 text-[13px] leading-[1.55] text-aurora-text-primary [overflow-wrap:anywhere] [&_a]:break-words [&_code]:break-words [&_li]:min-w-0 [&_ol]:min-w-0 [&_pre]:max-w-full [&_pre]:overflow-x-auto [&_pre]:[overflow-wrap:normal] [&_pre]:whitespace-pre [&_table]:block [&_table]:max-w-full [&_table]:overflow-x-auto [&_ul]:min-w-0">
      <Streamdown
        mode={isStreaming ? 'streaming' : 'static'}
        skipHtml
        rehypePlugins={NO_REHYPE_PLUGINS}
        disallowedElements={SAFE_MARKDOWN_IMAGE_ELEMENTS}
        allowElement={allowSafeMarkdownElement}
        urlTransform={safeMarkdownUrlTransform}
        controls={false}
        linkSafety={DISABLED_LINK_SAFETY}
        lineNumbers={false}
      >
        {text}
      </Streamdown>
      {isStreaming ? <StreamingCursor /> : null}
    </div>
  )
}

function AgentActionsPanel({
  open,
  onOpenChange,
  toolCalls,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  toolCalls: ACPMessage['toolCalls']
}) {
  return (
    <div className="w-full overflow-hidden rounded-aurora-3 border border-aurora-border-default bg-aurora-panel-medium shadow-[var(--aurora-shadow-strong),var(--aurora-highlight-strong)]">
      <Collapsible open={open} onOpenChange={onOpenChange}>
        <CollapsibleTrigger
          className="flex w-full items-center gap-2 px-4 py-3 text-[13px] font-medium text-aurora-text-muted transition-colors hover:text-aurora-text-primary"
        >
          <ListChecks className="size-4" />
          <span className="flex-1 text-left">Agent Actions</span>
          <span className="rounded-full border border-aurora-border-default bg-aurora-control-surface px-2 py-0.5 text-[10px] font-semibold text-aurora-text-muted">
            {toolCalls.length}
          </span>
          <ChevronDown className={cn('size-4 transition-transform', open ? 'rotate-180' : 'rotate-0')} />
        </CollapsibleTrigger>
        <CollapsibleContent className="px-4 pb-3">
          <div className="space-y-1">
            {groupConsecutiveToolCalls(toolCalls, getToolCategory).map((entry) =>
              entry.type === 'group' ? (
                <GroupedToolCallDisplay
                  key={`group-${entry.toolCalls[0]!.id}`}
                  category={entry.category}
                  toolCalls={entry.toolCalls}
                />
              ) : (
                <ToolCallDisplay key={entry.toolCall.id} toolCall={entry.toolCall} />
              ),
            )}
          </div>
        </CollapsibleContent>
      </Collapsible>
    </div>
  )
}

export function WorkingAssistantBubble({ label = 'Codex is working' }: { label?: string }) {
  return (
    <div className="group/bubble flex min-w-0 gap-3">
      <div className="mt-1 flex size-6 shrink-0 items-center justify-center rounded-full border border-aurora-accent-primary/30 bg-aurora-accent-deep/18">
        <Bot className="size-3 text-aurora-accent-primary" />
      </div>

      <div className="flex min-w-0 max-w-[calc(100%-2.25rem)] flex-col gap-2.5 sm:max-w-[80%]">
        <div
          role="status"
          aria-label={label}
          className="relative max-w-full overflow-hidden rounded-aurora-2 border border-aurora-border-default bg-aurora-panel-medium px-4 py-3 shadow-[var(--aurora-shadow-medium),var(--aurora-highlight-medium)]"
        >
          <span
            aria-hidden="true"
            className="absolute inset-y-0 left-0 w-[2px] rounded-l-aurora-2 bg-aurora-accent-primary/40"
          />

          <div className="flex min-w-0 items-center gap-2 pr-1 text-[13px] leading-[1.55] text-aurora-text-primary">
            <span className="font-medium">{label}</span>
            <span className="inline-flex shrink-0 items-center gap-1" aria-hidden="true">
              <span className="size-1.5 animate-pulse rounded-full bg-aurora-accent-primary motion-reduce:animate-none" />
              <span className="size-1.5 animate-pulse rounded-full bg-aurora-accent-primary/70 delay-150 motion-reduce:animate-none" />
              <span className="size-1.5 animate-pulse rounded-full bg-aurora-accent-primary/40 delay-300 motion-reduce:animate-none" />
            </span>
          </div>

          <div className="mt-3 space-y-1.5" aria-hidden="true">
            <div className="h-2 rounded-full bg-aurora-accent-primary/15" />
            <div className="h-2 rounded-full bg-aurora-accent-primary/10" />
            <div className="h-2 w-7/12 rounded-full bg-aurora-accent-primary/10" />
          </div>
        </div>
      </div>
    </div>
  )
}

function MessageBubbleComponent({ message }: { message: ACPMessage }) {
  const isUser = message.role === 'user'
  const isStreaming = Boolean(message.isStreaming)
  const [reasoningOpen, setReasoningOpen] = React.useState(isStreaming)
  const [chainOpen, setChainOpen] = React.useState(isStreaming)
  const [actionsOpen, setActionsOpen] = React.useState(isStreaming)

  React.useEffect(() => {
    setReasoningOpen(isStreaming)
    setChainOpen(isStreaming)
    setActionsOpen(isStreaming || message.toolCalls.length > 0)
  }, [isStreaming, message.toolCalls.length])

  const hasReasoning = !isUser && message.thoughts.length > 0
  const hasActions = !isUser && message.toolCalls.length > 0
  const messageContentClass = cn(
    MESSAGE_CONTENT_CLASS,
    isUser ? USER_MESSAGE_CONTENT_CLASS : ASSISTANT_MESSAGE_CONTENT_CLASS,
  )

  return (
    <div className={cn('group/bubble flex min-w-0 gap-3', isUser && 'flex-row-reverse')}>
      <div
        className={cn(
          'mt-1 flex size-6 shrink-0 items-center justify-center rounded-full border',
          isUser
            ? 'border-aurora-border-strong bg-aurora-panel-strong'
            : 'border-aurora-accent-primary/30 bg-aurora-accent-deep/18',
        )}
      >
        {isUser ? (
          <UserRound className="size-3 text-aurora-text-muted" />
        ) : (
          <Bot className="size-3 text-aurora-accent-primary" />
        )}
      </div>

      <div className={cn('flex min-w-0 max-w-[calc(100%-2.25rem)] flex-col gap-2.5 sm:max-w-[80%]', isUser && 'items-end')}>
        {hasReasoning && (
          <div className="w-full overflow-hidden rounded-aurora-3 border border-aurora-border-default bg-aurora-panel-medium shadow-[var(--aurora-shadow-strong),var(--aurora-highlight-strong)]">
            <ChainOfThought
              open={chainOpen}
              onOpenChange={setChainOpen}
              className="px-4 py-3"
            >
              <ChainOfThoughtHeader className="font-medium text-aurora-text-muted">
                Reasoning Summary
              </ChainOfThoughtHeader>
              <ChainOfThoughtContent className="pt-1">
                <div className="rounded-aurora-2 border border-aurora-border-default/70 bg-aurora-control-surface px-3 py-3">
                  <Reasoning
                    isStreaming={isStreaming}
                    open={reasoningOpen}
                    onOpenChange={setReasoningOpen}
                    className="mb-0"
                  >
                    <ReasoningTrigger
                      className="text-aurora-text-muted"
                      getThinkingMessage={(isStreaming, duration) => {
                        if (isStreaming || duration === 0) return <span className="animate-pulse">Reasoning…</span>
                        if (duration === undefined) return <span>Reasoning</span>
                        return <span>Reasoned for {duration} seconds</span>
                      }}
                    />
                    <ReasoningContent className="mt-3 space-y-3 text-aurora-text-primary">
                      {message.thoughts.join('\n\n')}
                    </ReasoningContent>
                  </Reasoning>
                </div>
              </ChainOfThoughtContent>
            </ChainOfThought>
          </div>
        )}

        {hasActions && (
          <AgentActionsPanel
            open={actionsOpen}
            onOpenChange={setActionsOpen}
            toolCalls={message.toolCalls}
          />
        )}

        {message.text && (
          <div className={messageContentClass}>
            {!isUser && (
              <span
                aria-hidden="true"
                className="absolute inset-y-0 left-0 w-[2px] rounded-l-aurora-2 bg-aurora-accent-primary/40"
              />
            )}
            {isUser ? (
              <p className="min-w-0 whitespace-pre-wrap pr-8 text-[13px] leading-[1.55] text-aurora-text-primary [overflow-wrap:anywhere]">
                {message.text}
                {message.isStreaming ? <StreamingCursor /> : null}
              </p>
            ) : (
              <AssistantMarkdown text={message.text} isStreaming={isStreaming} />
            )}
            <div className="absolute right-2 top-2">
              <CopyButton text={getMessageCopyText(message)} />
            </div>
          </div>
        )}
      </div>
    </div>
  )
}

function areMessageBubblePropsEqual(
  previous: Readonly<{ message: ACPMessage }>,
  next: Readonly<{ message: ACPMessage }>,
) {
  const prev = previous.message
  const current = next.message

  return (
    prev.id === current.id &&
    prev.role === current.role &&
    prev.text === current.text &&
    prev.isStreaming === current.isStreaming &&
    prev.version === current.version &&
    prev.thoughts.length === current.thoughts.length &&
    prev.toolCalls.length === current.toolCalls.length
  )
}

export const MessageBubble = React.memo(MessageBubbleComponent, areMessageBubblePropsEqual)
