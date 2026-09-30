'use client'

import { startTransition, useEffect, useRef, useState } from 'react'

export type DemoAsyncStatus = 'idle' | 'loading' | 'success' | 'error'

interface DemoAsyncCopy {
  idle: string
  loading: string
  success: string
  error: string
}

export function useDemoAsync(copy: DemoAsyncCopy, delayMs = 900) {
  const [status, setStatus] = useState<DemoAsyncStatus>('idle')
  const [message, setMessage] = useState(copy.idle)
  const timerRef = useRef<number | null>(null)

  useEffect(() => {
    return () => {
      if (timerRef.current !== null) {
        window.clearTimeout(timerRef.current)
      }
    }
  }, [])

  function run(outcome: Exclude<DemoAsyncStatus, 'idle' | 'loading'>) {
    if (timerRef.current !== null) {
      window.clearTimeout(timerRef.current)
    }

    setStatus('loading')
    setMessage(copy.loading)

    timerRef.current = window.setTimeout(() => {
      startTransition(() => {
        setStatus(outcome)
        setMessage(copy[outcome])
      })
    }, delayMs)
  }

  function reset() {
    if (timerRef.current !== null) {
      window.clearTimeout(timerRef.current)
    }

    setStatus('idle')
    setMessage(copy.idle)
  }

  return {
    status,
    message,
    run,
    reset,
  }
}
