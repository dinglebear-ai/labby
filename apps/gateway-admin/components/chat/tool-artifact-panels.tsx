'use client'

import * as React from 'react'
import type { ComponentProps } from 'react'
import { CheckCircle2, ExternalLink, Globe, ShieldAlert, XCircle } from 'lucide-react'

import { CodeBlock, CodeBlockCopyButton } from '@/components/ai/code-block'
import {
  Confirmation,
  ConfirmationAccepted,
  ConfirmationRejected,
  ConfirmationRequest,
  ConfirmationTitle,
} from '@/components/ai/confirmation'
import {
  FileTree,
  FileTreeFile,
  FileTreeFolder,
} from '@/components/ai/file-tree'
import { Source, Sources, SourcesContent, SourcesTrigger } from '@/components/ai/sources'
import {
  Terminal,
  TerminalActions,
  TerminalContent,
  TerminalCopyButton,
  TerminalHeader,
  TerminalTitle,
} from '@/components/ai/terminal'
import {
  WebPreview,
  WebPreviewBody,
  WebPreviewNavigation,
  WebPreviewNavigationButton,
  WebPreviewUrl,
} from '@/components/ai/web-preview'
import { AURORA_MUTED_LABEL } from '@/components/aurora/tokens'
import { cn } from '@/lib/utils'

import type { TranscriptToolCall } from './types'
import type { ToolArtifact } from './tool-call-presentation'

type FileNode = {
  key: string
  name: string
  path: string
  kind: 'file' | 'folder'
  children: FileNode[]
}

function classifyLanguage(path: string | null, code: string): string {
  const extension = path?.split('.').at(-1)?.toLowerCase()
  switch (extension) {
    case 'rs':
      return 'rust'
    case 'ts':
      return 'ts'
    case 'tsx':
      return 'tsx'
    case 'js':
      return 'js'
    case 'jsx':
      return 'jsx'
    case 'json':
      return 'json'
    case 'md':
      return 'markdown'
    case 'sh':
    case 'zsh':
    case 'bash':
      return 'bash'
    case 'yml':
    case 'yaml':
      return 'yaml'
    case 'toml':
      return 'toml'
    case 'html':
      return 'html'
    case 'css':
      return 'css'
    case 'py':
      return 'python'
    case 'go':
      return 'go'
    default:
      return code.trim().startsWith('{') || code.trim().startsWith('[') ? 'json' : 'markdown'
  }
}

function previewText(value: unknown): string | null {
  if (typeof value === 'string') {
    return value.trim() ? value : null
  }
  if (Array.isArray(value)) {
    const textValues = value.filter((entry): entry is string => typeof entry === 'string' && entry.trim().length > 0)
    if (textValues.length > 0) {
      return textValues.join('\n')
    }
  }
  if (value == null) {
    return null
  }
  if (typeof value === 'object') {
    const record = value as Record<string, unknown>
    const preferredKeys = ['stdout', 'stderr', 'raw_output', 'output', 'code', 'patch', 'diff', 'snippet', 'text', 'body']
    for (const key of preferredKeys) {
      const candidate = record[key]
      if (typeof candidate === 'string' && candidate.trim().length > 0) {
        return candidate
      }
      if (Array.isArray(candidate)) {
        const textValues = candidate.filter(
          (entry): entry is string => typeof entry === 'string' && entry.trim().length > 0,
        )
        if (textValues.length > 0) {
          return textValues.join('\n')
        }
      }
    }
  }
  try {
    return JSON.stringify(value, null, 2)
  } catch {
    return String(value)
  }
}

function canEmbedPreview(url: string) {
  try {
    const parsed = new URL(url)
    return (
      parsed.hostname === 'localhost' ||
      parsed.hostname === '127.0.0.1' ||
      parsed.hostname.endsWith('.local') ||
      parsed.protocol === 'file:'
    )
  } catch {
    return false
  }
}

function buildTree(paths: string[]): FileNode[] {
  const root: FileNode[] = []

  for (const fullPath of paths) {
    const parts = fullPath.split('/').filter(Boolean)
    let level = root
    let accumulated = ''

    parts.forEach((part, index) => {
      accumulated = `${accumulated}/${part}`
      const isLeaf = index === parts.length - 1
      let node = level.find((entry) => entry.key === accumulated)
      if (!node) {
        node = {
          key: accumulated,
          name: part,
          path: fullPath,
          kind: isLeaf ? 'file' : 'folder',
          children: [],
        }
        level.push(node)
      }
      if (!isLeaf) {
        level = node.children
      }
    })
  }

  function normalize(nodes: FileNode[]): FileNode[] {
    return [...nodes]
      .map((node) => ({ ...node, children: normalize(node.children) }))
      .sort((a, b) =>
        a.kind === b.kind ? a.name.localeCompare(b.name) : a.kind === 'folder' ? -1 : 1,
      )
  }

  return normalize(root)
}

function FileTreeNode({ node }: { node: FileNode }) {
  if (node.kind === 'folder') {
    return (
      <FileTreeFolder name={node.name} path={node.key}>
        {node.children.map((child) => (
          <FileTreeNode key={child.key} node={child} />
        ))}
      </FileTreeFolder>
    )
  }

  return <FileTreeFile name={node.name} path={node.path} />
}

function PermissionPanel({ toolCall }: { toolCall: TranscriptToolCall }) {
  const isResolved = Boolean(toolCall.permissionSelection)
  const granted =
    toolCall.permissionSelection === 'granted' ||
    toolCall.permissionSelection === 'allow_once' ||
    toolCall.permissionSelection === 'allow_always'

  return (
    <Confirmation
      approval={
        isResolved
          ? {
              id: toolCall.id,
              approved: granted,
              reason: toolCall.permissionSelection ?? undefined,
            }
          : { id: toolCall.id }
      }
      state={(isResolved ? 'approval-responded' : 'approval-requested') as never}
      className={cn(
        'mt-2 rounded-aurora-2 border px-3 py-3',
        isResolved
          ? granted
            ? 'border-aurora-success/40 bg-aurora-success/12'
            : 'border-aurora-error/40 bg-aurora-error/12'
          : 'border-aurora-warn/40 bg-aurora-warn/12',
      )}
    >
      <ConfirmationTitle className="block">
        <div className="flex items-start gap-2">
          {isResolved ? (
            granted ? (
              <CheckCircle2 className="mt-0.5 size-4 shrink-0 text-aurora-success" />
            ) : (
              <XCircle className="mt-0.5 size-4 shrink-0 text-aurora-error" />
            )
          ) : (
            <ShieldAlert className="mt-0.5 size-4 shrink-0 text-aurora-warn" />
          )}
          <div className="min-w-0 flex-1">
            <ConfirmationRequest>
              <div className="space-y-2">
                <p className="text-[12px] font-medium text-aurora-text-primary">Permission required</p>
                <p className="text-[12px] leading-[1.55] text-aurora-text-muted">{toolCall.title}</p>
                {toolCall.permissionOptions && toolCall.permissionOptions.length > 0 ? (
                  <div className="flex flex-wrap gap-2">
                    {toolCall.permissionOptions.map((option) => (
                      <span
                        key={option.optionId}
                        className="rounded-full border border-aurora-border-default bg-aurora-control-surface px-2.5 py-1 text-[11px] text-aurora-text-primary"
                      >
                        {option.name}
                      </span>
                    ))}
                  </div>
                ) : null}
              </div>
            </ConfirmationRequest>
            <ConfirmationAccepted>
              <div className="space-y-1">
                <p className="text-[12px] font-medium text-aurora-success">Permission granted</p>
                <p className="text-[12px] leading-[1.55] text-aurora-text-muted">{toolCall.title}</p>
              </div>
            </ConfirmationAccepted>
            <ConfirmationRejected>
              <div className="space-y-1">
                <p className="text-[12px] font-medium text-aurora-error">Permission rejected</p>
                <p className="text-[12px] leading-[1.55] text-aurora-text-muted">{toolCall.title}</p>
              </div>
            </ConfirmationRejected>
            {toolCall.permissionSelection ? (
              <p className={cn(AURORA_MUTED_LABEL, 'mt-2 text-aurora-text-muted/60')}>
                selection: {toolCall.permissionSelection}
              </p>
            ) : null}
          </div>
        </div>
      </ConfirmationTitle>
    </Confirmation>
  )
}

function SourcePanels({ links }: { links: string[] }) {
  if (links.length === 0) {
    return null
  }

  return (
    <Sources className="mt-2 rounded-aurora-2 border border-aurora-border-default/70 bg-aurora-control-surface px-3 py-2.5 text-aurora-text-primary">
      <SourcesTrigger
        count={links.length}
        className="w-full items-center justify-between text-[12px] font-medium text-aurora-text-primary"
      />
      <SourcesContent className="mt-2 w-full gap-2">
        {links.map((link) => (
          <Source
            key={link}
            href={link}
            title={link}
            className="rounded-aurora-1 border border-aurora-border-default/70 px-2.5 py-2 text-[12px] leading-[1.5] text-aurora-text-muted hover:border-aurora-accent-primary/40 hover:text-aurora-accent-primary"
          >
            <Globe className="size-3.5 shrink-0" />
            <span className="truncate">{link}</span>
            <ExternalLink className="ml-auto size-3 shrink-0" />
          </Source>
        ))}
      </SourcesContent>
    </Sources>
  )
}

function LocalPreview({ url }: { url: string }) {
  if (!canEmbedPreview(url)) {
    return null
  }

  return (
    <div className="mt-2 overflow-hidden rounded-aurora-2 border border-aurora-border-default/70 bg-aurora-control-surface">
      <WebPreview defaultUrl={url} className="border-0 bg-transparent">
        <WebPreviewNavigation className="border-aurora-border-default/60 bg-aurora-panel-medium px-2 py-2">
          <WebPreviewNavigationButton disabled tooltip="Back">
            <span className="sr-only">Back</span>
          </WebPreviewNavigationButton>
          <WebPreviewNavigationButton disabled tooltip="Forward">
            <span className="sr-only">Forward</span>
          </WebPreviewNavigationButton>
          <WebPreviewUrl
            className="h-8 border-aurora-border-default bg-aurora-control-surface text-[12px] text-aurora-text-primary"
            readOnly
          />
        </WebPreviewNavigation>
        <div className="h-56">
          <WebPreviewBody className="h-56 bg-aurora-page-bg" />
        </div>
      </WebPreview>
    </div>
  )
}

export function ToolArtifactPanels({
  toolCall,
  artifact,
}: {
  toolCall: TranscriptToolCall
  artifact: ToolArtifact
}) {
  const tree = React.useMemo(() => buildTree(artifact.fileTreePaths), [artifact.fileTreePaths])

  return (
    <>
      {toolCall.kind === 'permission' ? <PermissionPanel toolCall={toolCall} /> : null}
      {tree.length > 0 ? (
        <div className="mt-2 overflow-hidden rounded-aurora-2 border border-aurora-border-default/70 bg-aurora-control-surface">
          <div className="border-b border-aurora-border-default/60 px-2.5 py-2">
            <p className="text-[12px] font-medium text-aurora-text-primary">Touched files</p>
          </div>
          <FileTree
            defaultExpanded={new Set(tree.map((node) => node.key))}
            className="rounded-none border-0 bg-transparent px-0 py-0 text-[12px] text-aurora-text-primary"
          >
            {tree.map((node) => (
              <FileTreeNode key={node.key} node={node} />
            ))}
          </FileTree>
        </div>
      ) : null}

      {artifact.terminalOutput ? (
        // O6: bounded height prevents re-measure storm during streaming + virtualization.
        <div className="aurora-scrollbar mt-2 max-h-96 overflow-auto rounded-aurora-2 border border-aurora-border-default/70">
          <Terminal
            output={artifact.terminalOutput}
            className="rounded-none border-0 bg-aurora-page-bg text-aurora-text-primary"
          >
            <TerminalHeader className="border-b border-aurora-border-default/60 bg-aurora-panel-medium px-3 py-2">
              <TerminalTitle className="text-[12px] text-aurora-text-muted">Terminal output</TerminalTitle>
              <TerminalActions>
                <TerminalCopyButton className="size-7 text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary" />
              </TerminalActions>
            </TerminalHeader>
            <TerminalContent className="aurora-scrollbar max-h-72 bg-aurora-page-bg px-3 py-3 text-[11px] leading-[1.6]" />
          </Terminal>
        </div>
      ) : null}

      {artifact.codeBlock ? (
        <div className="mt-2 overflow-hidden rounded-aurora-2 border border-aurora-border-default/70">
          <CodeBlock
            code={artifact.codeBlock.code}
            language={(artifact.codeBlock.language ?? classifyLanguage(artifact.codeBlock.path, artifact.codeBlock.code)) as ComponentProps<typeof CodeBlock>['language']}
            className="rounded-none border-0 bg-aurora-control-surface"
          >
            <CodeBlockCopyButton className="size-7 rounded border border-aurora-border-default bg-aurora-panel-strong text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary" />
          </CodeBlock>
        </div>
      ) : null}

      <SourcePanels links={artifact.links} />
      {artifact.webPreviewUrl ? <LocalPreview url={artifact.webPreviewUrl} /> : null}
    </>
  )
}

export function ToolPayloadPreview({
  label,
  value,
  preferredPath,
}: {
  label: string
  value: unknown
  preferredPath?: string | null
}) {
  const text = previewText(value)
  if (!text) {
    return null
  }

  const escapeCharacter = String.fromCharCode(27)
  const isTerminal =
    text.startsWith('$ ') ||
    text.startsWith(escapeCharacter) ||
    text.includes('\n$ ') ||
    text.includes('\n> ')
  const looksLikeCode =
    preferredPath != null ||
    /(^\s*(function|const|let|class|export|import)\s)|([{};]\s*$)/m.test(text)

  return (
    <div className="border-b border-aurora-border-default/60 px-3 py-2 last:border-b-0">
      <p className={cn(AURORA_MUTED_LABEL, 'mb-1.5 text-aurora-text-muted/60')}>{label}</p>
      {isTerminal ? (
        <Terminal output={text} className="border-aurora-border-default/70 bg-aurora-page-bg">
          <TerminalHeader className="border-b border-aurora-border-default/60 bg-aurora-panel-medium px-3 py-2">
            <TerminalTitle className="text-[12px] text-aurora-text-muted">{label}</TerminalTitle>
            <TerminalActions>
              <TerminalCopyButton className="size-7 text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary" />
            </TerminalActions>
          </TerminalHeader>
          <TerminalContent className="aurora-scrollbar max-h-72 bg-aurora-page-bg px-3 py-3 text-[11px] leading-[1.6]" />
        </Terminal>
      ) : looksLikeCode ? (
        <CodeBlock
          code={text}
          language={classifyLanguage(preferredPath ?? null, text) as ComponentProps<typeof CodeBlock>['language']}
          className="border-aurora-border-default/70 bg-aurora-control-surface"
        >
          <CodeBlockCopyButton className="size-7 rounded border border-aurora-border-default bg-aurora-panel-strong text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary" />
        </CodeBlock>
      ) : (
        <div
          aria-label={`${label} plain text preview`}
          className="aurora-scrollbar max-h-72 overflow-auto rounded-aurora-1 border border-aurora-border-default/70 bg-aurora-page-bg px-3 py-2.5 text-[12px] leading-[1.6] whitespace-pre-wrap text-aurora-text-primary focus-visible:border-aurora-accent-primary focus-visible:ring-aurora-accent-primary/34 focus-visible:ring-[3px] focus-visible:outline-none"
          role="region"
          tabIndex={0}
        >
          {text}
        </div>
      )}
    </div>
  )
}
