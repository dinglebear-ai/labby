'use client'

import { FormEvent, useEffect, useRef, useState } from 'react'
import Link from 'next/link'
import { Activity, ArrowUpRight, Bot, Brain, File, Folder, MessagesSquare, Paperclip, PanelRight, Send, Settings, Square, Terminal, X } from 'lucide-react'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Button } from '@/components/ui/button'
import { useBrowserSession } from '@/lib/auth/session'
import { authorityIdentity } from '@/lib/auth/authority'
import { phoenixApi, phoenixSupports, type PhoenixAttachment, type PhoenixEvent, type PhoenixMessage, type PhoenixModel, type PhoenixSessionSummary, type PhoenixStatus } from '@/lib/api/phoenix-client'
import { Textarea } from '@/components/ui/textarea'
import { PhoenixRuntimeSummary, phoenixContextWindow, phoenixTotalTokens } from './phoenix-event-timeline'
import { PhoenixConversation } from './phoenix-conversation'
import { useOptionalConsoleShell } from './console-shell-context'

type PhoenixWindowRect = { x: number; y: number; width: number; height: number }

function PhoenixMark() {
  return <svg aria-hidden="true" width="25" height="25" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><circle cx="12" cy="3.6" r="1.05"/><path d="M12.9 4.5c.5.7.6 1.5.3 2.4M11.7 7.2C9.7 4.9 7 3.8 3.7 4.1c1.5 2.4 3.5 4.1 6 5M12.3 7.2c2-2.3 4.7-3.4 8-3.1-1.5 2.4-3.5 4.1-6 5M12 7.6c-1.4 1.3-2.2 3.2-2.5 5.6-.3 2.5.5 4.8 2.5 7 2-2.2 2.8-4.5 2.5-7-.3-2.4-1.1-4.3-2.5-5.6zM9.6 15.4 7.1 18.7M14.4 15.4l2.5 3.3M12 20.5v1.4"/></svg>
}

export function PhoenixAvailability() {
  const shell = useOptionalConsoleShell()
  const setPhoenixDocked = shell?.setPhoenixDocked
  const session = useBrowserSession()
  const identity = session.status === 'authenticated' ? authorityIdentity(session.authority) : session.status
  const [open, setOpen] = useState(false)
  const [status, setStatus] = useState<PhoenixStatus>()
  const [sessionId, setSessionId] = useState<string>()
  const [messages, setMessages] = useState<PhoenixMessage[]>([])
  const [events, setEvents] = useState<PhoenixEvent[]>([])
  const [models, setModels] = useState<PhoenixModel[]>([])
  const [model, setModel] = useState('')
  const [effort, setEffort] = useState('')
  const [attachments, setAttachments] = useState<PhoenixAttachment[]>([])
  const [input, setInput] = useState('')
  const [error, setError] = useState<string>()
  const [sending, setSending] = useState(false)
  const [interrupting, setInterrupting] = useState(false)
  const [steering, setSteering] = useState(false)
  const [workflowNotice, setWorkflowNotice] = useState<string>()
  const [loadingDiagnostics, setLoadingDiagnostics] = useState(false)
  const [copiedIndex, setCopiedIndex] = useState<number>()
  const [newThreadOnSend, setNewThreadOnSend] = useState(false)
  const [title, setTitle] = useState('Phoenix')
  const [editingTitle, setEditingTitle] = useState(false)
  const [dock, setDock] = useState<'float' | 'right'>('float')
  const [floatRect, setFloatRect] = useState<PhoenixWindowRect>()
  const [modelMenuOpen, setModelMenuOpen] = useState(false)
  const [reasoningMenuOpen, setReasoningMenuOpen] = useState(false)
  const [threadMenuOpen, setThreadMenuOpen] = useState(false)
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [threadHistory, setThreadHistory] = useState<PhoenixSessionSummary[]>([])
  const dragRef = useRef<{ pointerId: number; x: number; y: number; originX: number; originY: number } | undefined>(undefined)
  const resizeRef = useRef<{ pointerId: number; x: number; y: number; width: number; height: number } | undefined>(undefined)
  const animationFrameRef = useRef<number | undefined>(undefined)
  const scrollRef = useRef<HTMLDivElement>(null)
  const closeAfterTurnRef = useRef<string | undefined>(undefined)
  const requestGenerationRef = useRef(0)

  useEffect(() => {
    setPhoenixDocked?.(open && dock === 'right')
    return () => setPhoenixDocked?.(false)
  }, [dock, open, setPhoenixDocked])

  useEffect(() => {
    requestGenerationRef.current += 1
    setStatus(undefined)
    setSessionId(undefined)
    setMessages([])
    setEvents([])
    setModels([])
    setModel('')
    setEffort('')
    setAttachments([])
    setInput('')
    setError(undefined)
    setSending(false)
    setInterrupting(false)
    setCopiedIndex(undefined)
    // Identity changes and unmount revoke pending work; layout changes do not.
    return () => { requestGenerationRef.current += 1 }
  }, [identity])

  useEffect(() => {
    if (!open || session.status !== 'authenticated') return
    const controller = new AbortController()
    setError(undefined)
    void phoenixApi.status(controller.signal).then(async (nextStatus) => {
      setStatus(nextStatus)
      if (!nextStatus.available) return
      const catalog = await phoenixApi.models(controller.signal)
      const availableModels = catalog.models ?? []
      setModels(availableModels)
      const selected = availableModels.find((entry) => entry.isDefault) ?? availableModels[0]
      if (selected) { setModel(selected.model); setEffort(selected.defaultReasoningEffort) }
    }, (reason: unknown) => {
      if (!controller.signal.aborted) setError(reason instanceof Error ? reason.message : 'Phoenix status is unavailable')
    })
    return () => controller.abort()
  }, [identity, open, session.status])

  useEffect(() => {
    if (!open || status?.available !== true) return
    const controller = new AbortController()
    void phoenixApi.list(controller.signal).then((result) => setThreadHistory(result.sessions ?? []), () => undefined)
    return () => controller.abort()
  }, [open, status?.available])

  useEffect(() => {
    scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight, behavior: 'smooth' })
  }, [messages, sending])

  useEffect(() => {
    if (!sending || !sessionId) return
    const controller = new AbortController()
    let reading = false
    const refresh = async () => {
      if (reading) return
      reading = true
      try {
        const current = await phoenixApi.read(sessionId, controller.signal)
        if (!controller.signal.aborted) {
          setMessages(current.messages)
          setEvents(current.events ?? [])
        }
      } catch {
        // The pending turn request owns terminal errors; polling only streams progress.
      } finally {
        reading = false
      }
    }
    const timer = window.setInterval(() => { void refresh() }, 400)
    return () => { controller.abort(); window.clearInterval(timer) }
  }, [sending, sessionId])

  useEffect(() => {
    if (!open || dock !== 'float' || floatRect) return
    try {
      const saved = JSON.parse(window.localStorage.getItem('labby:phoenix-window') ?? 'null') as PhoenixWindowRect | null
      if (saved && saved.width > 0 && saved.height > 0) {
        const width = Math.min(Math.max(Math.min(380, window.innerWidth - 16), saved.width), window.innerWidth - 16)
        const height = Math.min(Math.max(Math.min(420, window.innerHeight - 16), saved.height), window.innerHeight - 16)
        setFloatRect({
          width,
          height,
          x: Math.min(Math.max(8, saved.x), Math.max(8, window.innerWidth - width - 8)),
          y: Math.min(Math.max(8, saved.y), Math.max(8, window.innerHeight - height - 8)),
        })
        return
      }
    } catch { /* Ignore malformed local window preferences. */ }
    const width = Math.min(window.innerWidth - 16, Math.min(760, Math.max(Math.min(420, window.innerWidth - 16), window.innerWidth * .48)))
    const height = Math.min(window.innerHeight - 16, Math.min(720, Math.max(Math.min(520, window.innerHeight - 16), window.innerHeight - 116)))
    setFloatRect({ x: Math.max(8, window.innerWidth - width - 22), y: Math.max(8, window.innerHeight - height - 92), width, height })
  }, [dock, floatRect, open])

  useEffect(() => {
    if (!floatRect) return
    try {
      window.localStorage.setItem('labby:phoenix-window', JSON.stringify(floatRect))
    } catch {
      // Window persistence is optional; Phoenix must remain usable when storage is unavailable.
    }
  }, [floatRect])

  useEffect(() => {
    const clamp = () => setFloatRect((current) => current ? {
      width: Math.min(current.width, window.innerWidth - 16),
      height: Math.min(current.height, window.innerHeight - 16),
      x: Math.min(Math.max(8, current.x), Math.max(8, window.innerWidth - Math.min(current.width, window.innerWidth - 16) - 8)),
      y: Math.min(Math.max(8, current.y), Math.max(8, window.innerHeight - Math.min(current.height, window.innerHeight - 16) - 8)),
    } : current)
    window.addEventListener('resize', clamp)
    return () => window.removeEventListener('resize', clamp)
  }, [])

  useEffect(() => {
    const move = (event: PointerEvent) => {
      const drag = dragRef.current
      const resize = resizeRef.current
      if (!drag && !resize) return
      if (animationFrameRef.current) cancelAnimationFrame(animationFrameRef.current)
      animationFrameRef.current = requestAnimationFrame(() => {
        if (drag) setFloatRect((current) => current ? {
          ...current,
          x: Math.min(window.innerWidth - current.width - 8, Math.max(8, drag.originX + event.clientX - drag.x)),
          y: Math.min(window.innerHeight - 56, Math.max(8, drag.originY + event.clientY - drag.y)),
        } : current)
        if (resize) setFloatRect((current) => current ? {
          ...current,
          width: Math.min(window.innerWidth - current.x - 8, Math.max(Math.min(380, window.innerWidth - 16), resize.width + event.clientX - resize.x)),
          height: Math.min(window.innerHeight - current.y - 8, Math.max(Math.min(420, window.innerHeight - 16), resize.height + event.clientY - resize.y)),
        } : current)
      })
    }
    const stop = () => { dragRef.current = undefined; resizeRef.current = undefined }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', stop)
    window.addEventListener('pointercancel', stop)
    return () => { if (animationFrameRef.current) cancelAnimationFrame(animationFrameRef.current); window.removeEventListener('pointermove', move); window.removeEventListener('pointerup', stop); window.removeEventListener('pointercancel', stop) }
  }, [])

  const sendTurn = async (draft: string, forceNewThread = false) => {
    const text = draft.trim()
    const outgoingAttachments = attachments
    if ((!text && outgoingAttachments.length === 0) || sending) return
    const displayText = text || `Attached: ${outgoingAttachments.map((attachment) => attachment.name).join(', ')}`
    const requestGeneration = ++requestGenerationRef.current
    setSending(true)
    setError(undefined)
    setInput('')
    setAttachments([])
    setMessages((current) => [...current, { role: 'user', text: displayText }])
    try {
      let activeSessionId = forceNewThread || newThreadOnSend ? undefined : sessionId
      if (!activeSessionId) {
        const started = await phoenixApi.start(model || undefined, effort || undefined)
        if (requestGeneration !== requestGenerationRef.current) return
        activeSessionId = started.session_id
        setSessionId(activeSessionId)
        setThreadHistory((current) => current.some((thread) => thread.session_id === started.session_id) ? current : [{ session_id: started.session_id, title: displayText.slice(0, 54), preview: displayText, model: model || null, effort: effort || null, message_count: 1, turn_status: 'in_progress' }, ...current])
      }
      const updated = await phoenixApi.send(activeSessionId, text, outgoingAttachments)
      if (requestGeneration !== requestGenerationRef.current) return
      setNewThreadOnSend(false)
      setMessages(updated.messages)
      setEvents(updated.events ?? [])
    } catch (reason) {
      if (requestGeneration !== requestGenerationRef.current) return
      setInput((current) => current ? [text, current].filter(Boolean).join('\n') : text)
      setAttachments((current) => [...outgoingAttachments, ...current].slice(0, 4))
      setMessages((current) => current.filter((message, index) => index !== current.length - 1 || message.role !== 'user' || message.text !== displayText))
      setError(reason instanceof Error ? reason.message : 'Phoenix could not complete the turn')
    } finally {
      if (requestGeneration === requestGenerationRef.current) {
        setSending(false)
        const closing = closeAfterTurnRef.current
        if (closing) {
          closeAfterTurnRef.current = undefined
          await phoenixApi.close(closing).catch(() => undefined)
          setSessionId(undefined)
        }
      }
    }
  }

  const submit = (event: FormEvent) => {
    event.preventDefault()
    if (sending && phoenixSupports(status, 'steer')) void steerTurn()
    else void sendTurn(input)
  }

  const steerTurn = async () => {
    const text = input.trim()
    const outgoingAttachments = attachments
    if (!sessionId || (!text && outgoingAttachments.length === 0) || steering) return
    const displayText = text || `Attached: ${outgoingAttachments.map((attachment) => attachment.name).join(', ')}`
    setSteering(true)
    setError(undefined)
    setInput('')
    setAttachments([])
    setMessages((current) => [...current, { role: 'user', text: displayText }])
    try {
      const updated = await phoenixApi.steer(sessionId, text, outgoingAttachments)
      const refreshed = await phoenixApi.read(sessionId).catch(() => undefined)
      if (refreshed) { setMessages(refreshed.messages); setEvents(refreshed.events ?? []) }
      setWorkflowNotice(updated.status === 'steered' ? 'Guidance added to the active turn' : undefined)
    } catch (reason) {
      setInput((current) => current ? [text, current].filter(Boolean).join('\n') : text)
      setAttachments((current) => [...outgoingAttachments, ...current].slice(0, 4))
      setMessages((current) => current.filter((message, index) => index !== current.length - 1 || message.role !== 'user' || message.text !== displayText))
      setError(reason instanceof Error ? reason.message : 'Phoenix could not steer the active turn')
    } finally {
      setSteering(false)
    }
  }

  const readDiagnostics = async () => {
    if (loadingDiagnostics) return
    setLoadingDiagnostics(true)
    setError(undefined)
    try {
      const result = await phoenixApi.diagnostics()
      const available = Object.entries(result).filter(([, value]) => value !== null && value !== undefined).map(([key]) => key.replaceAll('_', ' '))
      setWorkflowNotice(available.length ? `Diagnostics ready: ${available.join(', ')}` : 'No diagnostics were reported')
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : 'Phoenix diagnostics are unavailable')
    } finally {
      setLoadingDiagnostics(false)
    }
  }

  const addAttachments = async (files: FileList | null) => {
    if (!files?.length) return
    const selected = Array.from(files).slice(0, Math.max(0, 4 - attachments.length))
    const textLike = (file: File) => file.type.startsWith('text/') || /^(application\/(json|javascript|xml|yaml|x-yaml))$/.test(file.type) || /\.(md|txt|json|jsonl|ya?ml|toml|csv|ts|tsx|js|jsx|mjs|cjs|rs|py|go|java|kt|kts|sh|bash|zsh|fish|html?|css|scss|xml|sql|graphql|gql|ini|conf|log)$/i.test(file.name)
    const classify = (file: File): PhoenixAttachment['type'] | undefined => {
      if (/^image\/(png|jpeg|webp)$/.test(file.type) && file.size <= 5 * 1024 * 1024) return 'image'
      if (/^audio\/(mpeg|wav|mp4|webm)$/.test(file.type) && file.size <= 5 * 1024 * 1024) return 'audio'
      if (textLike(file) && file.size <= 512 * 1024) return 'text'
    }
    const accepted = selected.map((file) => ({ file, type: classify(file) })).filter((entry): entry is { file: File; type: PhoenixAttachment['type'] } => Boolean(entry.type))
    if (accepted.length !== selected.length) setError('Phoenix accepts PNG/JPEG/WebP images, supported audio, and UTF-8 text/code files up to 512 KiB. Binary files are not supported by Codex App Server input.')
    const encoded = await Promise.all(accepted.map(({ file, type }) => new Promise<PhoenixAttachment>((resolve, reject) => {
      const reader = new FileReader()
      reader.onload = () => {
        const raw = String(reader.result)
        const url = type === 'text' ? 'data:text/plain;base64,' + (raw.split(',', 2)[1] ?? '') : raw
        resolve({ type, url, name: file.name })
      }
      reader.onerror = reject
      reader.readAsDataURL(file)
    })))
    setAttachments((current) => [...current, ...encoded].slice(0, 4))
  }

  const closePanel = async () => {
    setOpen(false)
  }

  const closeThread = async (id: string) => {
    if (sending && id === sessionId) return
    await phoenixApi.close(id)
    setThreadHistory((current) => current.filter((thread) => thread.session_id !== id))
    if (id === sessionId) startNewThread()
  }

  const interruptTurn = async () => {
    if (!sessionId || !sending || interrupting) return
    setInterrupting(true)
    setError(undefined)
    try {
      const updated = await phoenixApi.interrupt(sessionId)
      setWorkflowNotice(updated.status === 'interrupting' ? 'Stopping the active turn' : undefined)
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : 'Phoenix could not stop the turn')
    } finally {
      setInterrupting(false)
    }
  }

  const retryFrom = (index: number) => {
    const userIndex = messages.slice(0, index + 1).findLastIndex((message) => message.role === 'user')
    if (userIndex < 0) return
    const text = messages[userIndex].text
    setMessages(messages.slice(0, userIndex))
    void sendTurn(text, true)
  }

  const copyMessage = async (text: string, index: number) => {
    await navigator.clipboard?.writeText(text)
    setCopiedIndex(index)
    window.setTimeout(() => setCopiedIndex((current) => current === index ? undefined : current), 1_500)
  }

  const switchThread = async (id: string) => {
    const requestGeneration = ++requestGenerationRef.current
    setSending(false)
    setError(undefined)
    try {
      const thread = await phoenixApi.read(id)
      if (requestGeneration !== requestGenerationRef.current) return
      setSessionId(id)
      setTitle(threadHistory.find((item) => item.session_id === id)?.title || 'Phoenix')
      setMessages(thread.messages)
      setEvents(thread.events ?? [])
      setThreadMenuOpen(false)
    } catch (reason) {
      if (requestGeneration !== requestGenerationRef.current) return
      setError(reason instanceof Error ? reason.message : 'Phoenix could not open that thread')
    }
  }

  const startNewThread = () => {
    requestGenerationRef.current += 1
    setSending(false)
    setSessionId(undefined)
    setMessages([])
    setEvents([])
    setInput('')
    setThreadMenuOpen(false)
  }

  const commitTitle = async () => {
    const nextTitle = title.trim() || 'Phoenix'
    const priorTitle = threadHistory.find((thread) => thread.session_id === sessionId)?.title || 'Phoenix'
    setTitle(nextTitle)
    setEditingTitle(false)
    if (!sessionId) return
    try {
      const renamed = await phoenixApi.rename(sessionId, nextTitle)
      setThreadHistory((current) => current.map((thread) => thread.session_id === sessionId ? renamed : thread))
    } catch (reason) {
      setTitle(priorTitle)
      setError(reason instanceof Error ? reason.message : 'Phoenix could not rename that thread')
    }
  }

  const available = session.status === 'authenticated' && status?.available === true
  const canInterrupt = status?.capabilities?.turn_lifecycle?.includes('interrupt') === true
  const canSteer = phoenixSupports(status, 'steer')
  const canReadDiagnostics = Boolean(status?.capabilities?.diagnostics?.length)
  const connecting = session.status === 'authenticated' && status === undefined && error === undefined
  const suggestions = ['Why is the reconcile sweep slow?', 'Fix the per-server backoff', 'Summarize today’s gateway errors']
  const selectedModel = models.find((entry) => entry.model === model)
  const usedTokens = phoenixTotalTokens(events) ?? 0
  const contextWindow = phoenixContextWindow(events) ?? selectedModel?.contextWindow
  const contextUsage = contextWindow ? Math.min(100, Math.max(0, Math.round(usedTokens / contextWindow * 100))) : undefined
  const contextLabel = contextUsage === undefined ? 'Context usage unavailable' : `Context usage ${contextUsage}% (${usedTokens.toLocaleString()} / ${contextWindow?.toLocaleString()} tokens)`
  const slashMatch = input.match(/(?:^|\s)\/[\w-]*$/)
  const mentionMatch = input.match(/(?:^|\s)@[\w./-]*$/)
  const completions = slashMatch ? [
    ['/ship', 'run the release checklist', 'COMMAND'], ['/scope-audit', 'least-privilege delta for this loadout', 'COMMAND'], ['/test', 'run the workspace test suite', 'COMMAND'], ['/diff', 'show the staged diff', 'COMMAND'], ['/clear', 'reset this conversation', 'COMMAND'],
  ] : mentionMatch ? [
    ['src/gateway/reconcile.rs', 'src/gateway/', 'FILE'], ['src/gateway/probe.rs', 'src/gateway/', 'FILE'], ['docs/', 'docs/', 'FOLDER'], ['rust-reviewer', 'Labby artifact', 'AGENT'], ['repo-triage', 'Labby artifact', 'SKILL'],
  ] : []
  const insertCompletion = (value: string) => setInput((current) => current.replace(/(?:\/|@)[\w./-]*$/, `${slashMatch ? '' : '@'}${value} `))
  const advertisedCapabilities = status?.capabilities ? [
    ...(status.capabilities.session_lifecycle ?? []),
    ...(status.capabilities.turn_lifecycle ?? []),
    ...(status.capabilities.inputs ?? []),
    ...(status.capabilities.operations ?? []),
    ...(status.capabilities.diagnostics ?? []),
  ].filter((value, index, values) => values.indexOf(value) === index && !status.capabilities?.unsupported?.includes(value)) : undefined
  return <Popover open={open} onOpenChange={setOpen}>
    <PopoverTrigger asChild><button type="button" aria-label={open ? 'Close Phoenix' : 'Ask Phoenix'} title={open ? 'Close Phoenix' : 'Ask Phoenix'} className={`fixed bottom-[50px] right-3 z-40 grid size-11 place-items-center rounded-full border border-aurora-border-strong bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] text-aurora-accent-pink shadow-[0_14px_34px_-12px_rgba(2,10,16,.66),inset_0_1px_0_rgba(255,255,255,.05)] transition-[right,transform,background-color,color] duration-150 hover:-translate-y-0.5 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-pink data-[state=open]:bg-aurora-accent-pink data-[state=open]:text-[#2a0f18] data-[state=open]:bg-none sm:bottom-[26px] sm:size-[52px] ${open && dock === 'right' ? 'sm:right-[442px]' : 'sm:right-[22px]'}`}><PhoenixMark/></button></PopoverTrigger>
    <PopoverContent data-phoenix-panel data-dock={dock} style={dock === 'float' && floatRect ? { left: floatRect.x, top: floatRect.y, width: floatRect.width, height: floatRect.height } : undefined} side="top" align="end" sideOffset={14} collisionPadding={8} aria-label="Phoenix session" className={`aurora-scrollbar !fixed flex flex-col overflow-hidden border-aurora-border-strong bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] p-0 text-aurora-text-primary shadow-[0_24px_60px_-12px_rgba(2,10,16,.72),0_0_0_1px_color-mix(in_srgb,var(--aurora-accent-pink)_16%,transparent)] ${dock === 'float' ? '!bottom-auto !right-auto max-h-[calc(100dvh-16px)] max-w-[calc(100vw-16px)] rounded-[16px] sm:min-h-[420px] sm:min-w-[380px]' : '!inset-y-0 !left-auto !right-0 h-dvh w-full rounded-none border-l sm:w-[min(420px,36vw)]'}`}>
      <div data-phoenix-drag-handle onPointerDown={(event) => { if (dock !== 'float' || !floatRect || (event.target as HTMLElement).closest('button,input,a')) return; event.currentTarget.setPointerCapture(event.pointerId); dragRef.current = { pointerId: event.pointerId, x: event.clientX, y: event.clientY, originX: floatRect.x, originY: floatRect.y } }} className={`relative flex shrink-0 select-none items-center gap-2.5 border-b border-aurora-border-default/60 bg-[linear-gradient(180deg,color-mix(in_srgb,var(--aurora-accent-pink)_7%,var(--gw0-0_38)),var(--gw0-0_38))] px-3 py-3 ${dock === 'float' ? 'touch-none cursor-grab active:cursor-grabbing' : ''}`}><span aria-hidden className="absolute inset-x-0 top-0 h-0.5 bg-[linear-gradient(90deg,transparent,color-mix(in_srgb,var(--aurora-accent-pink)_55%,transparent),transparent)]"/><span className="relative grid size-9 shrink-0 place-items-center rounded-[11px] border border-aurora-accent-pink/45 bg-aurora-accent-pink/10 text-aurora-accent-pink"><PhoenixMark/><span aria-label={available ? 'Phoenix available' : 'Phoenix unavailable'} className={`absolute -bottom-0.5 -right-0.5 size-2.5 rounded-full border-2 border-aurora-panel-strong ${available ? 'bg-aurora-status-success shadow-[0_0_7px_var(--aurora-success)]' : 'bg-aurora-text-muted'}`}/></span><div className="min-w-0 flex-1">{editingTitle ? <input autoFocus aria-label="Conversation title" value={title} onChange={(event) => setTitle(event.target.value)} onBlur={() => void commitTitle()} onKeyDown={(event) => { if (event.key === 'Enter') event.currentTarget.blur(); if (event.key === 'Escape') setEditingTitle(false) }} className="h-6 w-full max-w-[180px] rounded-md border border-aurora-border-strong bg-[var(--gw0-0_45)] px-1.5 font-display text-[15px] font-extrabold outline-none focus:border-aurora-accent-pink"/> : <h2 onClick={() => setEditingTitle(true)} title="Click to rename" className="truncate font-display text-[15px] font-extrabold">{title}</h2>}<div className="mt-0.5 flex min-w-0 gap-1 overflow-hidden"><span className="inline-flex h-5 shrink-0 items-center gap-1 rounded-md border border-aurora-accent-pink/35 bg-aurora-accent-pink/10 px-2 text-[10px] font-semibold text-aurora-text-muted"><Bot size={11} className="text-aurora-accent-pink"/>Codex</span><span className="inline-flex h-5 min-w-0 items-center gap-1 truncate rounded-md border border-aurora-accent-primary/35 bg-aurora-accent-primary/10 px-2 text-[10px] font-semibold text-aurora-text-muted"><Bot size={11} className="shrink-0 text-aurora-accent-primary"/>labby</span></div></div><div className="relative flex shrink-0 items-center gap-1"><button type="button" aria-expanded={threadMenuOpen} aria-label="Switch Phoenix thread" title="Threads" onClick={() => { setThreadMenuOpen(!threadMenuOpen); setSettingsOpen(false) }} className="grid size-8 place-items-center rounded-lg text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary"><MessagesSquare size={15}/></button><button type="button" aria-pressed={dock === 'right'} aria-label={dock === 'right' ? 'Float Phoenix panel' : 'Dock Phoenix right'} title={dock === 'right' ? 'Float Phoenix panel' : 'Dock Phoenix right'} onClick={() => setDock(dock === 'right' ? 'float' : 'right')} className="hidden size-8 place-items-center rounded-lg border border-aurora-accent-pink/45 text-aurora-accent-pink hover:bg-aurora-accent-pink/10 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary sm:grid"><PanelRight size={15}/></button><button type="button" aria-expanded={settingsOpen} aria-label="Phoenix settings" title="Phoenix settings" onClick={() => { setSettingsOpen(!settingsOpen); setThreadMenuOpen(false) }} className="grid size-8 place-items-center rounded-lg text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary"><Settings size={15}/></button><span className="mx-1 hidden h-5 w-px bg-aurora-border-default/60 sm:block"/><Button data-visible-label variant="ghost" size="icon" className="size-11 min-w-11 sm:size-8 sm:min-w-8" aria-label="Close Phoenix panel" onClick={() => void closePanel()}><X size={17}/></Button></div></div>
      {threadMenuOpen && <section aria-label="Phoenix threads" className="absolute right-12 top-[68px] z-30 w-[min(330px,calc(100%-24px))] rounded-xl border border-aurora-border-strong bg-aurora-panel-strong p-2 shadow-2xl"><div className="flex items-center justify-between px-2 py-1.5"><h3 className="text-[10px] font-bold uppercase tracking-[.14em] text-aurora-text-muted">Threads</h3><button type="button" onClick={startNewThread} className="inline-flex items-center gap-1 rounded-md px-2 py-1 text-[10.5px] font-bold text-aurora-accent-strong hover:bg-aurora-hover-bg"><span aria-hidden>+</span>New</button></div><div className="max-h-56 overflow-y-auto">{threadHistory.length === 0 ? <p className="px-2 py-5 text-center text-xs text-aurora-text-muted">No previous Phoenix threads</p> : threadHistory.map((thread) => <div key={thread.session_id} className="group/thread flex items-center rounded-lg hover:bg-aurora-hover-bg"><button type="button" aria-current={thread.session_id === sessionId ? 'true' : undefined} onClick={() => void switchThread(thread.session_id)} className="flex min-w-0 flex-1 items-center gap-2 rounded-lg px-2.5 py-2 text-left aria-[current=true]:bg-aurora-accent-pink/10"><span className="grid size-7 shrink-0 place-items-center rounded-lg border border-aurora-accent-pink/30 text-aurora-accent-pink"><PhoenixMark/></span><span className="min-w-0 flex-1 truncate text-xs font-semibold">{thread.title}</span></button><button type="button" aria-label={`Close ${thread.title}`} title="Close thread" disabled={sending && thread.session_id === sessionId} onClick={() => void closeThread(thread.session_id)} className="mr-1 grid size-7 place-items-center rounded-lg text-aurora-text-muted opacity-0 hover:bg-aurora-hover-bg hover:text-aurora-status-error group-hover/thread:opacity-100 focus-visible:opacity-100 disabled:opacity-30"><X size={13}/></button></div>)}</div></section>}
      {settingsOpen && <section aria-label="Phoenix settings menu" className="absolute right-10 top-[68px] z-30 w-[min(330px,calc(100%-24px))] rounded-xl border border-aurora-border-strong bg-aurora-panel-strong p-3 shadow-2xl"><h3 className="mb-2 text-[10px] font-bold uppercase tracking-[.14em] text-aurora-text-muted">Phoenix settings</h3><div className="space-y-2 text-xs"><div className="flex justify-between gap-4"><span className="text-aurora-text-muted">Runtime</span><strong>Container-local Codex</strong></div><div className="flex justify-between gap-4"><span className="text-aurora-text-muted">Labby MCP</span><strong className={status?.mcp?.configured ? 'text-aurora-status-success' : 'text-aurora-warn'}>{status?.mcp?.configured ? 'Connected' : 'Unavailable'}</strong></div><div className="flex justify-between gap-4"><span className="text-aurora-text-muted">Access</span><strong>Read only</strong></div></div>{canReadDiagnostics && <button type="button" aria-label="Read Phoenix diagnostics" disabled={loadingDiagnostics} onClick={() => void readDiagnostics()} className="mt-3 flex w-full items-center justify-center gap-1.5 rounded-lg border border-aurora-border-default bg-aurora-control-surface px-3 py-2 text-xs font-bold hover:border-aurora-accent-primary"><Activity size={13}/>{loadingDiagnostics ? 'Reading…' : 'Read diagnostics'}</button>}<PhoenixRuntimeSummary mcpConfigured={status?.mcp?.configured} protocol={status?.protocol?.schema} runtimeVersion={status?.protocol?.runtime_version} capabilities={advertisedCapabilities} unsupported={status?.capabilities?.unsupported}/><Link href="/settings/" onClick={() => setSettingsOpen(false)} className="mt-3 inline-flex w-full items-center justify-center gap-1.5 rounded-lg border border-aurora-border-default bg-aurora-control-surface px-3 py-2 text-xs font-bold text-aurora-text-primary hover:border-aurora-accent-primary"><Settings size={13}/>Open Labby settings</Link></section>}
      {connecting ? <div role="status" className="m-auto flex items-center gap-2 rounded-[11px] border border-aurora-accent-pink/30 bg-aurora-accent-pink/[.06] px-4 py-3 text-[11.5px] font-bold text-aurora-accent-pink"><span className="motion-safe:animate-pulse"><PhoenixMark/></span>Connecting to Codex App Server</div> : available ? <>
        <div ref={scrollRef} role="log" aria-live="polite" className="aurora-scrollbar flex flex-1 flex-col gap-3 overflow-y-auto px-4 py-4">
          {messages.length === 0 && <div className="flex shrink-0 flex-col gap-[9px] px-0.5 py-1.5"><p className="text-[12.5px] leading-[1.6] text-aurora-text-muted">Attached to the container-local session. Ask a question, or start with one of these.</p>{suggestions.map((suggestion) => <button key={suggestion} type="button" onClick={() => void sendTurn(suggestion)} className="w-full rounded-[10px] border border-aurora-border-default/45 bg-[var(--gw0-0_40)] px-[11px] py-[9px] text-left text-xs font-semibold text-aurora-text-primary transition-colors hover:border-aurora-accent-pink/50 hover:bg-aurora-accent-pink/[.07] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-pink">{suggestion}</button>)}</div>}
          <PhoenixConversation messages={messages} events={events} mark={<PhoenixMark/>} copiedIndex={copiedIndex} onRetry={retryFrom} onCopy={(text, index) => { void copyMessage(text, index) }} onEdit={(index, text) => { setInput(text); setMessages(messages.slice(0, index)); setNewThreadOnSend(true) }}/>
          {workflowNotice && <p role="status" className="shrink-0 rounded-[9px] border border-aurora-accent-primary/25 bg-aurora-accent-primary/[.06] px-2.5 py-2 text-[10.5px] font-semibold text-aurora-accent-strong">{workflowNotice}</p>}
          {sending && <div role="status" className="flex shrink-0 items-center gap-[9px] rounded-[11px] border border-aurora-accent-pink/30 bg-aurora-accent-pink/[.06] px-[11px] py-2"><span className="text-aurora-accent-pink motion-safe:animate-pulse"><PhoenixMark/></span><span className="text-[11.5px] font-bold text-aurora-accent-pink">Working</span><span className="flex gap-[3px]">{[0, 1, 2].map((dot) => <span key={dot} className="size-1 rounded-full bg-aurora-accent-pink motion-safe:animate-[phoenixDot_1.1s_ease-in-out_infinite]" style={{ animationDelay: `${dot * .18}s` }}/>)}</span></div>}
        </div>
        <form onSubmit={submit} className="relative shrink-0 border-t border-aurora-border-default/60 bg-[linear-gradient(180deg,var(--gw0-0_38),color-mix(in_srgb,var(--aurora-accent-pink)_4%,var(--gw0-0_38)))] px-[11px] pb-[11px] pt-3 before:absolute before:inset-x-0 before:top-0 before:h-px before:bg-[linear-gradient(90deg,transparent,color-mix(in_srgb,var(--aurora-accent-pink)_30%,transparent),transparent)]">
          {error && <p role="alert" className="mb-2 text-xs text-aurora-status-error">{error}</p>}
          {completions.length > 0 && <div role="listbox" aria-label={slashMatch ? 'Phoenix commands' : 'Phoenix mentions'} className="absolute inset-x-3 bottom-[108px] z-30 max-h-[310px] overflow-y-auto rounded-xl border border-aurora-border-strong bg-aurora-panel-strong p-2 shadow-2xl"><div className="flex items-center justify-between px-2 py-1.5"><strong className="text-[10px] uppercase tracking-[.14em] text-aurora-text-muted">{slashMatch ? 'Commands' : 'Mentions'}</strong><span className="text-[10px] text-aurora-text-muted">↵ insert · esc dismiss</span></div>{completions.map(([value, detail, badge], index) => <button type="button" role="option" aria-selected={index === 0} key={value} onClick={() => insertCompletion(value)} className="flex w-full items-center gap-3 rounded-lg px-2.5 py-2 text-left hover:bg-aurora-hover-bg aria-[selected=true]:bg-aurora-accent-primary/10"><span className="grid size-7 shrink-0 place-items-center rounded-lg border border-aurora-accent-primary/40 text-aurora-accent-strong">{slashMatch ? <Terminal size={14}/> : badge === 'FOLDER' ? <Folder size={14}/> : badge === 'FILE' ? <File size={14}/> : <Bot size={14}/>}</span><span className="min-w-0 flex-1"><strong className="block truncate text-[12.5px]">{value}</strong><small className="block truncate text-[11px] text-aurora-text-muted">{detail}</small></span><span className="rounded-md border border-aurora-border-default px-2 py-1 text-[9px] font-bold tracking-wide text-aurora-text-muted">{badge}</span></button>)}</div>}
          {attachments.length > 0 && <div aria-label="Phoenix attachments" className="mb-2 flex gap-1 overflow-x-auto">{attachments.map((attachment, index) => <button type="button" title="Remove attachment" key={`${attachment.name}-${index}`} onClick={() => setAttachments((current) => current.filter((_, itemIndex) => itemIndex !== index))} className="max-w-[150px] truncate rounded-md border border-aurora-accent-pink/30 bg-aurora-accent-pink/[.07] px-2 py-1 text-[9.5px] text-aurora-text-primary">{attachment.name} ×</button>)}</div>}
          <div className="relative mb-2 flex items-center gap-2"><div aria-label={contextLabel} title={contextLabel} className="flex h-7 items-center gap-2 rounded-lg border border-aurora-border-default bg-aurora-control-surface px-2.5"><span className="h-1.5 w-12 overflow-hidden rounded-full bg-[var(--gw0-0_70)]"><span className="block h-full rounded-full bg-aurora-accent-primary transition-[width]" style={{ width: `${contextUsage ?? 0}%` }}/></span><strong className="min-w-[2.4rem] text-right text-[11px] tabular-nums text-aurora-text-muted">{contextUsage === undefined ? '—' : `${contextUsage}%`}</strong></div><span className="flex-1"/><span className="h-5 w-px bg-aurora-border-default"/>{models.length > 0 && <><button type="button" aria-label="Choose Phoenix model" title={selectedModel?.displayName ?? 'Choose model'} aria-expanded={modelMenuOpen} onClick={() => { setModelMenuOpen(!modelMenuOpen); setReasoningMenuOpen(false) }} className="grid size-8 place-items-center rounded-lg border border-aurora-border-default bg-aurora-control-surface text-[13px] font-extrabold text-aurora-text-primary hover:border-aurora-accent-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-strong disabled:opacity-60">AI</button><button type="button" aria-label="Choose Phoenix reasoning" title={effort || 'Choose reasoning'} aria-expanded={reasoningMenuOpen} onClick={() => { setReasoningMenuOpen(!reasoningMenuOpen); setModelMenuOpen(false) }} className="grid size-8 place-items-center rounded-lg border border-aurora-border-default bg-aurora-control-surface text-aurora-text-primary hover:border-aurora-accent-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-strong disabled:opacity-60"><Brain size={16}/></button></>}{modelMenuOpen && <div role="menu" aria-label="Phoenix models" className="absolute bottom-10 right-10 z-40 w-64 rounded-xl border border-aurora-border-strong bg-aurora-panel-strong p-1.5 shadow-2xl">{models.map((entry) => <button type="button" role="menuitemradio" aria-checked={entry.model === model} key={entry.id} onClick={() => { setModel(entry.model); setEffort(entry.defaultReasoningEffort); setNewThreadOnSend(Boolean(sessionId)); setModelMenuOpen(false) }} className="flex w-full gap-3 rounded-lg px-3 py-2 text-left hover:bg-aurora-hover-bg aria-[checked=true]:bg-aurora-accent-pink/10"><span className="pt-0.5 text-sm font-extrabold">AI</span><span><strong className={entry.model === model ? 'text-aurora-accent-pink' : 'text-aurora-text-primary'}>{entry.displayName}</strong><small className="block text-[11px] text-aurora-text-muted">{entry.description}</small></span></button>)}</div>}{reasoningMenuOpen && <div role="menu" aria-label="Phoenix reasoning efforts" className="absolute bottom-10 right-0 z-40 w-56 rounded-xl border border-aurora-border-strong bg-aurora-panel-strong p-1.5 shadow-2xl">{selectedModel?.supportedReasoningEfforts.map((option) => <button type="button" role="menuitemradio" aria-checked={option.reasoningEffort === effort} key={option.reasoningEffort} onClick={() => { setEffort(option.reasoningEffort); setNewThreadOnSend(Boolean(sessionId)); setReasoningMenuOpen(false) }} className="block w-full rounded-lg px-3 py-2 text-left hover:bg-aurora-hover-bg aria-[checked=true]:bg-aurora-accent-pink/10"><strong className={option.reasoningEffort === effort ? 'text-aurora-accent-pink' : 'text-aurora-text-primary'}>{option.reasoningEffort}</strong><small className="block text-[11px] text-aurora-text-muted">{option.description}</small></button>)}</div>}</div>
          <div className="flex items-end gap-2"><div className="flex min-w-0 flex-1 items-end rounded-[13px] border border-aurora-border-strong bg-aurora-control-surface px-2 py-1.5 shadow-[inset_0_1px_0_rgba(255,255,255,.04)] transition-shadow focus-within:border-aurora-accent-pink/70 focus-within:shadow-[0_0_0_1px_color-mix(in_srgb,var(--aurora-accent-pink)_45%,transparent)]"><label title="Attach image or file" className="grid size-[34px] shrink-0 cursor-pointer place-items-center text-aurora-text-muted hover:text-aurora-accent-pink"><Paperclip size={15}/><input aria-label="Attach image or file" type="file" accept="image/png,image/jpeg,image/webp,audio/mpeg,audio/wav,audio/mp4,audio/webm,text/*,application/json,application/javascript,application/xml,application/yaml,.md,.jsonl,.toml,.yaml,.yml,.ts,.tsx,.js,.jsx,.rs,.py,.go,.java,.kt,.sh,.sql,.graphql" multiple className="sr-only" onChange={(event) => { void addAttachments(event.target.files); event.currentTarget.value = '' }}/></label><Textarea aria-label="Message Phoenix" value={input} onChange={(event) => setInput(event.target.value)} onPaste={(event) => { if (event.clipboardData.files.length) { event.preventDefault(); void addAttachments(event.clipboardData.files) } }} onKeyDown={(event) => { if (event.key === 'Escape' && completions.length) { event.preventDefault(); setInput((current) => current.replace(/(?:\/|@)[\w./-]*$/, '')) } else if (event.key === 'Enter' && !event.shiftKey) { event.preventDefault(); if (completions[0]) insertCompletion(completions[0][0]); else event.currentTarget.form?.requestSubmit() } }} placeholder={sending ? 'Message Phoenix while it works…' : 'Ask Phoenix anything'} className="min-h-[38px] max-h-[132px] resize-none border-0 bg-transparent px-1 py-2 text-[13px] font-medium leading-[1.5] shadow-none focus-visible:ring-0"/></div>{sending && canSteer && <Button type="submit" size="icon" aria-label="Steer Phoenix" disabled={(!input.trim() && attachments.length === 0) || steering} className="size-[42px] min-w-[42px] rounded-[11px] border border-aurora-accent-primary/60 bg-aurora-control-surface text-aurora-accent-strong"><Send size={15}/></Button>}{sending && canInterrupt ? <Button type="button" size="icon" className="size-[42px] min-w-[42px] rounded-[11px] border border-aurora-accent-pink/70 bg-aurora-accent-pink text-[#2a0f18]" disabled={!sessionId || interrupting} aria-label={interrupting ? 'Stopping Phoenix' : 'Stop Phoenix'} onClick={() => void interruptTurn()}><Square size={13} fill="currentColor"/></Button> : <Button type="submit" size="icon" className="size-[42px] min-w-[42px] rounded-[11px] border border-aurora-accent-pink/70 bg-aurora-accent-pink text-[#2a0f18] shadow-[0_4px_14px_-4px_color-mix(in_srgb,var(--aurora-accent-pink)_55%,transparent)] disabled:border-aurora-border-strong/70 disabled:bg-aurora-control-surface disabled:text-aurora-text-muted disabled:shadow-none" disabled={sending || (!input.trim() && attachments.length === 0)} aria-label="Send message"><Send size={15}/></Button>}</div>
        </form>
      </> : <>
        <div className="flex flex-1 flex-col justify-center gap-3 px-6 py-8"><h3 className="font-display text-lg font-bold">Session execution is unavailable</h3><p className="text-[12.5px] leading-relaxed text-aurora-text-muted">{error ?? 'Phoenix needs the container-local Codex App Server and its isolated account to be configured by an operator.'}</p><Link href="/agents" onClick={() => setOpen(false)} className="inline-flex items-center gap-1 self-start rounded-md py-1 text-xs font-semibold text-aurora-accent-strong hover:underline focus-visible:ring-2 focus-visible:ring-aurora-accent-primary">View agents<ArrowUpRight size={13}/></Link></div>
        <div className="border-t border-aurora-border-subtle p-3"><p className="rounded-[12px] border border-aurora-border-default bg-[var(--gw0-0_40)] px-3 py-3 text-xs text-aurora-text-muted">Messaging becomes available when the in-container service is connected.</p></div>
      </>}
      {dock === 'float' && floatRect && <button type="button" aria-label="Resize Phoenix panel" title="Drag to resize" onPointerDown={(event) => { event.preventDefault(); event.currentTarget.setPointerCapture(event.pointerId); resizeRef.current = { pointerId: event.pointerId, x: event.clientX, y: event.clientY, width: floatRect.width, height: floatRect.height } }} className="absolute bottom-0 right-0 z-20 size-7 cursor-se-resize touch-none rounded-tl-xl text-transparent after:absolute after:bottom-1.5 after:right-1.5 after:size-2.5 after:border-b-2 after:border-r-2 after:border-aurora-border-strong hover:after:border-aurora-accent-pink focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-aurora-accent-pink">Resize</button>}
    </PopoverContent>
  </Popover>
}

export function ConsoleGlobalTools() {
  return <PhoenixAvailability/>
}
