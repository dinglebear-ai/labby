'use client'

import { useCallback, useEffect, useRef, useState, useSyncExternalStore, type ComponentType } from 'react'
import { usePathname } from 'next/navigation'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { isCommandK, OPEN_COMMAND_PALETTE_EVENT } from '@/lib/command-palette-events'
import { getBrowserSessionContextIdentity, subscribeToBrowserSession } from '@/lib/auth/session-store'
import type { AppCommandPaletteProps } from '@/components/app-command-palette'

type PaletteModule = { AppCommandPalette: ComponentType<AppCommandPaletteProps> }
const loadPalette = () => import('@/components/app-command-palette')

/** Keep the closed palette and its catalog graph off the initial route. */
export function AppCommandPaletteEntry({ load = loadPalette }: { load?: () => Promise<PaletteModule> }) {
  const pathname = usePathname()
  const workspaceIdentity = useSyncExternalStore(subscribeToBrowserSession, getBrowserSessionContextIdentity, getBrowserSessionContextIdentity)
  const [Palette, setPalette] = useState<ComponentType<AppCommandPaletteProps> | null>(null)
  const [wantedOpen, setWantedOpen] = useState(false)
  const [failed, setFailed] = useState(false)
  const intent = useRef(false)
  const requestedPathname = useRef(pathname)
  const requestedWorkspaceIdentity = useRef(workspaceIdentity)
  const currentPathname = useRef(pathname)
  currentPathname.current = pathname
  const pending = useRef(false)
  const mounted = useRef(false)
  const origin = useRef<HTMLElement | null>(null)

  useEffect(() => {
    mounted.current = true
    return () => { mounted.current = false }
  }, [])

  const cancel = useCallback((restoreFocus = true) => {
    intent.current = false
    setWantedOpen(false)
    setFailed(false)
    const opener = origin.current
    origin.current = null
    if (restoreFocus && opener?.isConnected) opener.focus()
  }, [])

  const request = useCallback((retry = false) => {
    if (!retry) origin.current = document.activeElement instanceof HTMLElement ? document.activeElement : null
    intent.current = true
    requestedPathname.current = pathname
    requestedWorkspaceIdentity.current = workspaceIdentity
    setWantedOpen(true)
    setFailed(false)
    if (pending.current) return
    pending.current = true
    load().then(module => {
      pending.current = false
      if (!mounted.current) return
      if (requestedPathname.current !== currentPathname.current || requestedWorkspaceIdentity.current !== getBrowserSessionContextIdentity()) {
        intent.current = false
        setWantedOpen(false)
      }
      // Radix records this element for focus restoration when the dialog closes.
      if (intent.current && origin.current?.isConnected) origin.current.focus()
      setPalette(() => module.AppCommandPalette)
    }).catch(() => {
      pending.current = false
      if (mounted.current && intent.current) setFailed(true)
    })
  }, [load, pathname, workspaceIdentity])

  useEffect(() => {
    if (Palette) return
    function keydown(event: KeyboardEvent) {
      if (isCommandK(event)) {
        event.preventDefault()
        if (intent.current) cancel()
        else request()
      } else if (event.key === 'Escape' && intent.current) {
        event.preventDefault()
        cancel()
      }
    }
    const open = () => request()
    window.addEventListener('keydown', keydown)
    window.addEventListener(OPEN_COMMAND_PALETTE_EVENT, open)
    return () => {
      window.removeEventListener('keydown', keydown)
      window.removeEventListener(OPEN_COMMAND_PALETTE_EVENT, open)
    }
  }, [Palette, cancel, request])

  const previousContext = useRef({ pathname, workspaceIdentity })
  useEffect(() => {
    if ((previousContext.current.pathname !== pathname || previousContext.current.workspaceIdentity !== workspaceIdentity) && !Palette) cancel(false)
    previousContext.current = { pathname, workspaceIdentity }
  }, [pathname, workspaceIdentity, Palette, cancel])

  if (Palette) return <Palette initialOpen={wantedOpen && requestedPathname.current === pathname && requestedWorkspaceIdentity.current === workspaceIdentity} />
  if (!wantedOpen) return null
  return (
    <div className="fixed inset-x-4 top-16 z-50 mx-auto max-w-sm">
      <Alert role={failed ? 'alert' : 'status'} variant={failed ? 'error' : 'default'}>
        <AlertDescription>
          <p>{failed ? 'The command palette could not load. Try again.' : 'Loading command palette…'}</p>
          <div className="flex gap-2">
            {failed ? <Button size="sm" variant="outline" onClick={() => request(true)}>Try again</Button> : null}
            <Button size="sm" variant="ghost" onClick={() => cancel()}>Cancel</Button>
          </div>
        </AlertDescription>
      </Alert>
    </div>
  )
}
