'use client'

import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { AppBridge, PostMessageTransport } from '@modelcontextprotocol/ext-apps/app-bridge'
import type { McpResourceReadResult } from '@/lib/api/phoenix-client'

export type McpAppResourceReader = (params: { uri: string }, signal?: AbortSignal) => Promise<McpResourceReadResult>

export function selectMcpAppHtml(result: McpResourceReadResult, resourceUri: string): string | null {
  const content = result.contents?.find((entry) => {
    if (typeof entry.text !== 'string') return false
    const mime = entry.mimeType ?? entry.mime_type
    return mime ? mime.toLowerCase().includes('html') : entry.uri === resourceUri
  })
  return content?.text ?? null
}

function bridgeToolResult(value: unknown): Parameters<AppBridge['sendToolResult']>[0] {
  if (value === undefined) return { content: [] }
  if (value !== null && typeof value === 'object' && !Array.isArray(value)) {
    const result = value as Record<string, unknown>
    if (Array.isArray(result.content)) return value as Parameters<AppBridge['sendToolResult']>[0]
    return { content: [], structuredContent: result }
  }
  return { content: [], structuredContent: { value } }
}

export function mcpAppFallbackText(value: unknown): string | null {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) return null
  const content = (value as Record<string, unknown>).content
  if (!Array.isArray(content)) return null
  const text = content.flatMap((entry) => {
    if (entry === null || typeof entry !== 'object' || Array.isArray(entry)) return []
    const candidate = (entry as Record<string, unknown>).text
    return typeof candidate === 'string' && candidate.trim() ? [candidate] : []
  }).join('\n')
  return text || null
}

export function McpAppResourcePanel({ resourceUri, appName, toolInput = {}, toolResult, readResource }: { resourceUri: string; appName?: string; toolInput?: Record<string, unknown>; toolResult?: unknown; readResource: McpAppResourceReader }) {
  const [html, setHtml] = useState<string | null>(null)
  const [state, setState] = useState<'loading' | 'ready' | 'unavailable' | 'error'>('loading')
  const iframeRef = useRef<HTMLIFrameElement>(null)
  const bridgeRef = useRef<AppBridge | null>(null)
  const toolInputRef = useRef(toolInput)
  const toolResultRef = useRef(toolResult)
  toolInputRef.current = toolInput
  toolResultRef.current = toolResult
  const fallbackText = mcpAppFallbackText(toolResult)

  useEffect(() => {
    const controller = new AbortController()
    let cancelled = false
    setHtml(null)
    setState('loading')
    readResource({ uri: resourceUri }, controller.signal).then((result) => {
      if (cancelled) return
      const nextHtml = selectMcpAppHtml(result, resourceUri)
      if (nextHtml) {
        setHtml(nextHtml)
        setState('ready')
      } else setState('unavailable')
    }).catch(() => { if (!cancelled) setState('error') })
    return () => { cancelled = true; controller.abort() }
  }, [readResource, resourceUri])

  useEffect(() => () => { void bridgeRef.current?.close() }, [])

  useLayoutEffect(() => {
    const iframe = iframeRef.current
    if (!iframe || !html) return
    let active = true
    let initialized = false
    const fail = () => {
      if (!active) return
      void bridgeRef.current?.close()
      bridgeRef.current = null
      setHtml(null)
      setState('error')
    }
    iframe.addEventListener('error', fail)
    const frameWindow = iframe.contentWindow
    if (!frameWindow) return () => iframe.removeEventListener('error', fail)
    void bridgeRef.current?.close()
    const bridge = new AppBridge(
      null,
      { name: 'Labby Web', version: '1.0.0' },
      { serverResources: {} },
      { hostContext: { displayMode: 'inline', platform: 'web' } },
    )
    bridgeRef.current = bridge
    bridge.onreadresource = async (params, extra) => readResource(
      { uri: params.uri },
      extra.signal,
    ) as never
    bridge.oninitialized = () => {
      initialized = true
      void bridge.sendToolInput({ arguments: toolInputRef.current })
        .then(() => bridge.sendToolResult(bridgeToolResult(toolResultRef.current)))
        .catch(fail)
    }
    bridge.connect(new PostMessageTransport(frameWindow, frameWindow))
      .catch(fail)
    iframe.srcdoc = html
    const timeout = window.setTimeout(() => { if (!initialized) fail() }, 10_000)
    return () => {
      active = false
      window.clearTimeout(timeout)
      iframe.removeEventListener('error', fail)
      if (bridgeRef.current === bridge) {
        bridgeRef.current = null
        void bridge.close()
      }
    }
  }, [html, readResource])

  return <section data-mcp-app={resourceUri} className="mt-2 overflow-hidden rounded-[10px] border border-aurora-border-default bg-aurora-panel-strong">
    <div className="flex items-center gap-2 border-b border-aurora-border-default px-3 py-2 text-xs"><strong>MCP App</strong><span className="truncate text-aurora-text-muted">{appName ?? resourceUri}</span></div>
    <div className="min-h-[220px] bg-white">
      {html ? <iframe ref={iframeRef} title={`${resourceUri} MCP UI`} className="block min-h-[320px] w-full border-0 bg-white" sandbox="allow-scripts allow-forms allow-popups allow-downloads"/> : <div className="flex min-h-[220px] items-center justify-center px-5 text-center text-xs text-[#4a6872]">{state === 'loading' ? 'Loading MCP App…' : state === 'error' ? 'Failed to load MCP App resource.' : 'MCP App resource unavailable.'}</div>}
    </div>
    {fallbackText ? <div data-mcp-app-fallback className="border-t border-aurora-border-default px-3 py-2 whitespace-pre-wrap text-xs text-aurora-text-secondary">{fallbackText}</div> : null}
  </section>
}
