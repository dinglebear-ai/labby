'use client'

import { useCallback, useLayoutEffect, useRef, useState } from 'react'
import type { Gateway, TestGatewayResult } from '@/lib/types/gateway'

type ProbeResult = { gateway: Gateway; result: TestGatewayResult }
type Probe = (id: string, signal?: AbortSignal) => Promise<TestGatewayResult>

/** One operator-selected probe owns the panel, loading state, and completion toasts. */
export function useGatewayProbe(probe: Probe, identity?: string | null) {
  const request = useRef<AbortController | null>(null)
  const [state, setState] = useState<{ identity: typeof identity; result: ProbeResult | null; pending: boolean }>({ identity, result: null, pending: false })
  const cancel = useCallback(() => {
    const previous = request.current
    request.current = null
    previous?.abort()
  }, [])

  useLayoutEffect(() => {
    setState({ identity, result: null, pending: false })
    return cancel
  }, [identity, cancel])

  const close = useCallback(() => {
    cancel()
    setState({ identity, result: null, pending: false })
  }, [identity, cancel])

  const run = useCallback(async (gateway: Gateway) => {
    cancel()
    const controller = new AbortController()
    request.current = controller
    setState(previous => ({ identity, result: previous.identity === identity ? previous.result : null, pending: true }))
    const current = () => request.current === controller && !controller.signal.aborted
    try {
      const result = await probe(gateway.id, controller.signal)
      if (!current()) return
      setState({ identity, result: { gateway, result }, pending: false })
      return result
    } catch (error) {
      if (current()) throw error
    } finally {
      if (current()) {
        request.current = null
        setState(previous => ({ ...previous, pending: false }))
      }
    }
  }, [probe, identity, cancel])

  return { run, close, result: state.identity === identity ? state.result : null, isTesting: state.identity === identity && state.pending }
}
