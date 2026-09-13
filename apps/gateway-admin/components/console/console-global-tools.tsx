'use client'

import { FormEvent, useEffect, useRef, useState } from 'react'
import Link from 'next/link'
import { usePathname } from 'next/navigation'
import { ArrowUpRight, Bot, Box, Check, Clipboard, PanelLeft, PanelRight, Pencil, RefreshCw, Send, ShieldCheck, X } from 'lucide-react'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Button } from '@/components/ui/button'
import { useBrowserSession } from '@/lib/auth/session'
import { authorityIdentity } from '@/lib/auth/authority'
import { skillLibrary } from '@/lib/api/skill-library-client'
import { gatewayApi, gatewayAction } from '@/lib/api/gateway-client'
import { snippetsApi } from '@/lib/api/snippets-client'
import { phoenixApi, type PhoenixMessage, type PhoenixStatus } from '@/lib/api/phoenix-client'
import { Textarea } from '@/components/ui/textarea'
import type { BackendGatewayMcpRuntimeView } from '@/lib/server/gateway-adapter'
import { deriveConsoleStatus } from './console-status-strip'

const TRAY_DESTINATIONS = [
  ['artifacts', '/library', 'Artifacts'], ['loadouts', '/loadouts', 'Loadouts'],
  ['snippets', '/snippets', 'Snippets'], ['tools', '/tools', 'Tools'],
] as const

type TrayCounts = Partial<Record<(typeof TRAY_DESTINATIONS)[number][0], number>>

function useTrayCounts() {
  const session = useBrowserSession()
  const identity = session.status === 'authenticated' ? authorityIdentity(session.authority) : session.status
  const [result, setResult] = useState<{ identity: string; counts: TrayCounts }>({ identity, counts: {} })
  useEffect(() => {
    const controller = new AbortController()
    let timer: ReturnType<typeof setTimeout> | undefined
    if (session.status !== 'authenticated') return () => controller.abort()
    const refresh = async () => {
      const signal = controller.signal
      const results = await Promise.allSettled([
        skillLibrary.list('', signal),
        gatewayApi.listLoadouts(signal), snippetsApi.list(signal),
        gatewayAction<BackendGatewayMcpRuntimeView[]>('gateway.mcp.list', {}, signal),
      ])
      if (signal.aborted) return
      const counts: TrayCounts = {}
      const [artifacts, loadouts, snippets, tools] = results
      if (artifacts.status === 'fulfilled' && !artifacts.value.next_cursor) counts.artifacts = artifacts.value.items.length
      if (loadouts.status === 'fulfilled') counts.loadouts = loadouts.value.length
      if (snippets.status === 'fulfilled') counts.snippets = snippets.value.length
      if (tools.status === 'fulfilled') counts.tools = deriveConsoleStatus(tools.value).tools
      setResult({ identity, counts })
      timer = setTimeout(() => { void refresh() }, 30_000)
    }
    void refresh()
    return () => { controller.abort(); clearTimeout(timer) }
  }, [identity, session.status])
  return result.identity === identity && session.status === 'authenticated' ? result.counts : {}
}

export function ConsoleLibraryTray({ counts }: { counts: TrayCounts }) {
  const pathname = usePathname() ?? ''
  return <nav aria-label="Quick library navigation" data-console-library-tray="1" className="aurora-scrollbar flex shrink-0 gap-0.5 overflow-x-auto border-t border-aurora-border-subtle bg-[var(--gw0-0_30)] px-5 pr-20">
    {TRAY_DESTINATIONS.map(([key, href, label]) => <Link key={key} href={href} aria-current={pathname === href || pathname.startsWith(`${href}/`) ? 'page' : undefined} className="inline-flex h-[38px] shrink-0 items-center gap-2 whitespace-nowrap border-b-2 border-transparent px-3.5 text-[12.5px] font-[650] text-aurora-text-muted hover:text-aurora-text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-aurora-accent-primary aria-[current=page]:border-aurora-accent-primary aria-[current=page]:text-aurora-accent-strong">
      {label}<span title={counts[key] === undefined ? 'Count unavailable for the current authority' : undefined} className="inline-flex h-[19px] min-w-5 items-center justify-center rounded-[5px] border border-aurora-border-strong/60 bg-[var(--gw0-0_45)] px-[5px] text-[10.5px] font-bold tabular-nums">{counts[key] ?? '—'}</span>
    </Link>)}
  </nav>
}

function PhoenixMark() {
  return <svg aria-hidden="true" width="25" height="25" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><circle cx="12" cy="3.6" r="1.05"/><path d="M12.9 4.5c.5.7.6 1.5.3 2.4M11.7 7.2C9.7 4.9 7 3.8 3.7 4.1c1.5 2.4 3.5 4.1 6 5M12.3 7.2c2-2.3 4.7-3.4 8-3.1-1.5 2.4-3.5 4.1-6 5M12 7.6c-1.4 1.3-2.2 3.2-2.5 5.6-.3 2.5.5 4.8 2.5 7 2-2.2 2.8-4.5 2.5-7-.3-2.4-1.1-4.3-2.5-5.6zM9.6 15.4 7.1 18.7M14.4 15.4l2.5 3.3M12 20.5v1.4"/></svg>
}

export function PhoenixAvailability() {
  const session = useBrowserSession()
  const identity = session.status === 'authenticated' ? authorityIdentity(session.authority) : session.status
  const [open, setOpen] = useState(false)
  const [status, setStatus] = useState<PhoenixStatus>()
  const [sessionId, setSessionId] = useState<string>()
  const [messages, setMessages] = useState<PhoenixMessage[]>([])
  const [input, setInput] = useState('')
  const [error, setError] = useState<string>()
  const [sending, setSending] = useState(false)
  const [copiedIndex, setCopiedIndex] = useState<number>()
  const [title, setTitle] = useState('Phoenix')
  const [editingTitle, setEditingTitle] = useState(false)
  const [dock, setDock] = useState<'float' | 'left' | 'right'>('float')
  const [floatOffset, setFloatOffset] = useState({ x: 0, y: 0 })
  const dragRef = useRef<{ x: number; y: number; originX: number; originY: number } | undefined>(undefined)
  const scrollRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    setStatus(undefined)
    setSessionId(undefined)
    setMessages([])
    setInput('')
    setError(undefined)
    setSending(false)
    setCopiedIndex(undefined)
  }, [identity])

  useEffect(() => {
    if (!open || session.status !== 'authenticated') return
    const controller = new AbortController()
    setError(undefined)
    void phoenixApi.status(controller.signal).then(setStatus, (reason: unknown) => {
      if (!controller.signal.aborted) setError(reason instanceof Error ? reason.message : 'Phoenix status is unavailable')
    })
    return () => controller.abort()
  }, [identity, open, session.status])

  useEffect(() => {
    scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight, behavior: 'smooth' })
  }, [messages, sending])

  useEffect(() => {
    const move = (event: PointerEvent) => {
      const drag = dragRef.current
      if (!drag) return
      setFloatOffset({ x: drag.originX + event.clientX - drag.x, y: drag.originY + event.clientY - drag.y })
    }
    const stop = () => { dragRef.current = undefined }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', stop)
    window.addEventListener('pointercancel', stop)
    return () => { window.removeEventListener('pointermove', move); window.removeEventListener('pointerup', stop); window.removeEventListener('pointercancel', stop) }
  }, [])

  const sendTurn = async (draft: string) => {
    const text = draft.trim()
    if (!text || sending) return
    setSending(true)
    setError(undefined)
    setInput('')
    setMessages((current) => [...current, { role: 'user', text }])
    try {
      let activeSessionId = sessionId
      if (!activeSessionId) {
        const started = await phoenixApi.start()
        activeSessionId = started.session_id
        setSessionId(activeSessionId)
      }
      const updated = await phoenixApi.send(activeSessionId, text)
      setMessages(updated.messages)
    } catch (reason) {
      setInput(text)
      setMessages((current) => current.filter((message, index) => index !== current.length - 1 || message.role !== 'user' || message.text !== text))
      setError(reason instanceof Error ? reason.message : 'Phoenix could not complete the turn')
    } finally {
      setSending(false)
    }
  }

  const submit = (event: FormEvent) => {
    event.preventDefault()
    void sendTurn(input)
  }

  const retryFrom = (index: number) => {
    const userIndex = messages.slice(0, index + 1).findLastIndex((message) => message.role === 'user')
    if (userIndex < 0) return
    const text = messages[userIndex].text
    setMessages(messages.slice(0, userIndex))
    void sendTurn(text)
  }

  const copyMessage = async (text: string, index: number) => {
    await navigator.clipboard?.writeText(text)
    setCopiedIndex(index)
    window.setTimeout(() => setCopiedIndex((current) => current === index ? undefined : current), 1_500)
  }

  const available = session.status === 'authenticated' && status?.available === true
  const connecting = session.status === 'authenticated' && status === undefined && error === undefined
  const suggestions = ['Summarize gateway health', 'Show what needs attention', 'Explain the latest failures']
  return <Popover open={open} onOpenChange={setOpen}>
    <PopoverTrigger asChild><button type="button" aria-label={open ? 'Close Phoenix' : 'Ask Phoenix'} title={open ? 'Close Phoenix' : 'Ask Phoenix'} className="fixed bottom-[50px] right-3 z-40 grid size-11 place-items-center rounded-full border border-aurora-border-strong bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] text-aurora-accent-pink shadow-[0_14px_34px_-12px_rgba(2,10,16,.66),inset_0_1px_0_rgba(255,255,255,.05)] transition-[transform,background-color,color] duration-150 hover:-translate-y-0.5 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-pink data-[state=open]:bg-aurora-accent-pink data-[state=open]:text-[#2a0f18] data-[state=open]:bg-none sm:bottom-[26px] sm:right-[22px] sm:size-[52px]"><PhoenixMark/></button></PopoverTrigger>
    <PopoverContent data-phoenix-panel data-dock={dock} style={dock === 'float' ? { translate: `${floatOffset.x}px ${floatOffset.y}px` } : undefined} side="top" align="end" sideOffset={14} collisionPadding={8} aria-label="Phoenix session" className={`aurora-scrollbar flex w-[min(420px,calc(100vw-16px))] flex-col overflow-hidden border-aurora-border-strong bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] p-0 text-aurora-text-primary shadow-[0_24px_60px_-12px_rgba(2,10,16,.72),0_0_0_1px_color-mix(in_srgb,var(--aurora-accent-pink)_16%,transparent)] motion-safe:animate-in motion-safe:fade-in motion-safe:duration-200 ${dock === 'float' ? 'h-[min(560px,calc(100dvh-112px))] origin-bottom-right rounded-[16px] motion-safe:slide-in-from-bottom-2' : `!fixed !inset-y-0 h-dvh !translate-x-0 !translate-y-0 rounded-none ${dock === 'left' ? '!left-0 !right-auto border-r' : '!left-auto !right-0 border-l'}`}`}>
      <div onPointerDown={(event) => { if (dock !== 'float' || (event.target as HTMLElement).closest('button,input')) return; dragRef.current = { x: event.clientX, y: event.clientY, originX: floatOffset.x, originY: floatOffset.y } }} className={`relative flex shrink-0 select-none items-center gap-2.5 border-b border-aurora-border-default/60 bg-[linear-gradient(180deg,color-mix(in_srgb,var(--aurora-accent-pink)_7%,var(--gw0-0_38)),var(--gw0-0_38))] py-[11px] pl-[13px] pr-2 ${dock === 'float' ? 'cursor-grab active:cursor-grabbing' : ''}`}><span aria-hidden className="absolute inset-x-0 top-0 h-0.5 bg-[linear-gradient(90deg,transparent,color-mix(in_srgb,var(--aurora-accent-pink)_55%,transparent),transparent)]"/><span className="relative grid size-[30px] shrink-0 place-items-center rounded-[10px] border border-aurora-accent-pink/40 bg-[linear-gradient(180deg,color-mix(in_srgb,var(--aurora-accent-pink)_16%,transparent),color-mix(in_srgb,var(--aurora-accent-pink)_7%,transparent))] text-aurora-accent-pink shadow-[inset_0_1px_0_rgba(255,255,255,.08)]"><PhoenixMark/><span aria-label={available ? 'Phoenix available' : 'Phoenix unavailable'} className={`absolute -bottom-0.5 -right-0.5 size-2 rounded-full border-2 border-aurora-panel-strong ${available ? 'bg-aurora-status-success shadow-[0_0_5px_var(--aurora-success)]' : 'bg-aurora-text-muted'}`}/></span><div className="min-w-0 flex-1">{editingTitle ? <input autoFocus aria-label="Conversation title" value={title} onChange={(event) => setTitle(event.target.value)} onBlur={() => { setTitle(title.trim() || 'Phoenix'); setEditingTitle(false) }} onKeyDown={(event) => { if (event.key === 'Enter') event.currentTarget.blur(); if (event.key === 'Escape') setEditingTitle(false) }} className="h-[21px] w-full max-w-[150px] rounded-[5px] border border-aurora-border-strong bg-[var(--gw0-0_45)] px-1.5 font-display text-[13.5px] font-extrabold outline-none focus:border-aurora-accent-pink"/> : <h2 onDoubleClick={() => setEditingTitle(true)} title="Double-click to rename" className="truncate font-display text-[13.5px] font-extrabold">{title}</h2>}<div className="mt-0.5 flex min-w-0 gap-1 overflow-hidden"><span className="inline-flex h-[17px] shrink-0 items-center gap-1 rounded-[5px] border border-aurora-accent-pink/25 bg-aurora-accent-pink/10 px-1.5 text-[9.5px] font-semibold text-aurora-text-muted"><Bot size={10} className="text-aurora-accent-pink"/>Codex</span><span className="inline-flex h-[17px] min-w-0 items-center gap-1 truncate rounded-[5px] border border-aurora-accent-primary/25 bg-aurora-accent-primary/10 px-1.5 text-[9.5px] font-semibold text-aurora-text-muted"><Box size={10} className="shrink-0 text-aurora-accent-primary"/>App Server</span><span className="inline-flex h-[17px] shrink-0 items-center gap-1 rounded-[5px] border border-aurora-status-success/25 bg-aurora-status-success/10 px-1.5 text-[9.5px] font-semibold text-aurora-text-muted"><ShieldCheck size={10} className="text-aurora-status-success"/>Read only</span></div></div><div className="flex shrink-0 items-center gap-0.5"><button type="button" aria-pressed={dock === 'left'} aria-label={dock === 'left' ? 'Float Phoenix panel' : 'Dock Phoenix left'} onClick={() => setDock(dock === 'left' ? 'float' : 'left')} className="hidden size-[25px] place-items-center rounded-[7px] text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary sm:grid"><PanelLeft size={13}/></button><button type="button" aria-pressed={dock === 'right'} aria-label={dock === 'right' ? 'Float Phoenix panel' : 'Dock Phoenix right'} onClick={() => setDock(dock === 'right' ? 'float' : 'right')} className="hidden size-[25px] place-items-center rounded-[7px] text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary sm:grid"><PanelRight size={13}/></button><span className="mx-1 hidden h-[15px] w-px bg-aurora-border-default/60 sm:block"/><Button data-visible-label variant="ghost" size="icon" className="size-11 min-w-11 sm:size-8 sm:min-w-8" aria-label="Close Phoenix panel" onClick={() => setOpen(false)}><X size={16}/></Button></div></div>
      {connecting ? <div role="status" className="m-auto flex items-center gap-2 rounded-[11px] border border-aurora-accent-pink/30 bg-aurora-accent-pink/[.06] px-4 py-3 text-[11.5px] font-bold text-aurora-accent-pink"><span className="motion-safe:animate-pulse"><PhoenixMark/></span>Connecting to Codex App Server</div> : available ? <>
        <div ref={scrollRef} role="log" aria-live="polite" className="aurora-scrollbar flex flex-1 flex-col gap-3 overflow-y-auto px-4 py-4">
          {messages.length === 0 && <div className="flex shrink-0 flex-col gap-[9px] px-0.5 py-1.5"><p className="text-[12.5px] leading-[1.6] text-aurora-text-muted">Attached to the container-local session. Ask a question, or start with one of these.</p>{suggestions.map((suggestion) => <button key={suggestion} type="button" onClick={() => void sendTurn(suggestion)} className="w-full rounded-[10px] border border-aurora-border-default/45 bg-[var(--gw0-0_40)] px-[11px] py-[9px] text-left text-xs font-semibold text-aurora-text-primary transition-colors hover:border-aurora-accent-pink/50 hover:bg-aurora-accent-pink/[.07] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-pink">{suggestion}</button>)}</div>}
          {messages.map((message, index) => message.role === 'user' ? <div data-phoenix-message="user" key={`${message.role}-${index}`} className="group flex shrink-0 flex-col items-end gap-[3px]"><div className="max-w-[84%] whitespace-pre-wrap rounded-[12px_12px_3px_12px] border border-aurora-accent-pink/30 bg-[color-mix(in_srgb,var(--aurora-accent-pink)_12%,var(--aurora-control-surface))] px-3 py-[9px] text-[12.5px] leading-[1.6] text-aurora-text-primary">{message.text}</div><div className="flex min-h-5 items-center gap-1 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100"><button type="button" onClick={() => { setInput(message.text); setMessages(messages.slice(0, index)) }} aria-label="Edit message" className="grid size-5 place-items-center rounded-md text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary"><Pencil size={11}/></button><button type="button" onClick={() => retryFrom(index)} aria-label="Retry from here" className="grid size-5 place-items-center rounded-md text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-accent-pink"><RefreshCw size={11}/></button><button type="button" onClick={() => void copyMessage(message.text, index)} aria-label="Copy message" className="grid size-5 place-items-center rounded-md text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary">{copiedIndex === index ? <Check size={11}/> : <Clipboard size={11}/>}</button></div></div> : <div data-phoenix-message="assistant" key={`${message.role}-${index}`} className="group flex min-w-0 shrink-0 gap-[9px]"><span className="grid size-[26px] shrink-0 place-items-center rounded-[9px] border border-aurora-accent-pink/40 bg-aurora-accent-pink/10 text-aurora-accent-pink"><PhoenixMark/></span><div className="min-w-0 flex-1"><div className="whitespace-pre-wrap text-[12.5px] font-semibold leading-[1.68] text-aurora-text-primary">{message.text}</div><div className="flex min-h-5 gap-1 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100"><button type="button" onClick={() => retryFrom(index)} aria-label="Regenerate" className="grid size-5 place-items-center rounded-md text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-accent-pink"><RefreshCw size={11}/></button><button type="button" onClick={() => void copyMessage(message.text, index)} aria-label="Copy answer" className="grid size-5 place-items-center rounded-md text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary">{copiedIndex === index ? <Check size={11}/> : <Clipboard size={11}/>}</button></div></div></div>)}
          {sending && <div role="status" className="flex shrink-0 items-center gap-[9px] rounded-[11px] border border-aurora-accent-pink/30 bg-aurora-accent-pink/[.06] px-[11px] py-2"><span className="text-aurora-accent-pink motion-safe:animate-pulse"><PhoenixMark/></span><span className="text-[11.5px] font-bold text-aurora-accent-pink">Working</span><span className="flex gap-[3px]">{[0, 1, 2].map((dot) => <span key={dot} className="size-1 rounded-full bg-aurora-accent-pink motion-safe:animate-[phoenixDot_1.1s_ease-in-out_infinite]" style={{ animationDelay: `${dot * .18}s` }}/>)}</span></div>}
        </div>
        <form onSubmit={submit} className="relative shrink-0 border-t border-aurora-border-default/60 bg-[linear-gradient(180deg,var(--gw0-0_38),color-mix(in_srgb,var(--aurora-accent-pink)_4%,var(--gw0-0_38)))] px-[11px] pb-[11px] pt-3 before:absolute before:inset-x-0 before:top-0 before:h-px before:bg-[linear-gradient(90deg,transparent,color-mix(in_srgb,var(--aurora-accent-pink)_30%,transparent),transparent)]">
          {error && <p role="alert" className="mb-2 text-xs text-aurora-status-error">{error}</p>}
          <div className="mb-2 flex items-center gap-1.5 text-[9.5px] font-semibold text-aurora-text-muted"><span className="h-5 rounded-md border border-aurora-border-default/60 bg-[var(--gw0-0_45)] px-2 leading-5">Container local</span><span className="h-5 rounded-md border border-aurora-status-success/25 bg-aurora-status-success/[.08] px-2 leading-5 text-aurora-status-success">Read only</span><span className="flex-1"/><span>Enter to send · Shift Enter for line break</span></div>
          <div className="flex items-end gap-2"><div className="flex min-w-0 flex-1 items-end rounded-[13px] border border-aurora-border-strong bg-aurora-control-surface px-2 py-1.5 shadow-[inset_0_1px_0_rgba(255,255,255,.04)] transition-shadow focus-within:border-aurora-accent-pink/70 focus-within:shadow-[0_0_0_1px_color-mix(in_srgb,var(--aurora-accent-pink)_45%,transparent)]"><Textarea aria-label="Message Phoenix" value={input} onChange={(event) => setInput(event.target.value)} onKeyDown={(event) => { if (event.key === 'Enter' && !event.shiftKey) { event.preventDefault(); event.currentTarget.form?.requestSubmit() } }} disabled={sending} placeholder={sending ? 'Working…' : 'Ask Phoenix anything'} className="min-h-[34px] max-h-[132px] resize-none border-0 bg-transparent px-1 py-[7px] text-[12.5px] font-medium leading-[1.5] shadow-none focus-visible:ring-0"/></div><Button type="submit" size="icon" className="size-11 min-w-11 rounded-[10px] border border-aurora-accent-pink/70 bg-aurora-accent-pink text-[#2a0f18] shadow-[0_4px_14px_-4px_color-mix(in_srgb,var(--aurora-accent-pink)_55%,transparent)] hover:bg-aurora-accent-pink/90 disabled:border-aurora-border-strong/70 disabled:bg-aurora-control-surface disabled:text-aurora-text-muted disabled:shadow-none sm:size-[34px] sm:min-w-[34px]" disabled={sending || !input.trim()} aria-label="Send message"><Send size={14}/></Button></div>
        </form>
      </> : <>
        <div className="flex flex-1 flex-col justify-center gap-3 px-6 py-8"><h3 className="font-display text-lg font-bold">Session execution is unavailable</h3><p className="text-[12.5px] leading-relaxed text-aurora-text-muted">{error ?? 'Phoenix needs the container-local Codex App Server and its isolated account to be configured by an operator.'}</p><Link href="/agents" onClick={() => setOpen(false)} className="inline-flex items-center gap-1 self-start rounded-md py-1 text-xs font-semibold text-aurora-accent-strong hover:underline focus-visible:ring-2 focus-visible:ring-aurora-accent-primary">View agents<ArrowUpRight size={13}/></Link></div>
        <div className="border-t border-aurora-border-subtle p-3"><p className="rounded-[12px] border border-aurora-border-default bg-[var(--gw0-0_40)] px-3 py-3 text-xs text-aurora-text-muted">Messaging becomes available when the in-container service is connected.</p></div>
      </>}
    </PopoverContent>
  </Popover>
}

export function ConsoleGlobalTools() {
  const counts = useTrayCounts()
  return <><ConsoleLibraryTray counts={counts}/><PhoenixAvailability/></>
}
