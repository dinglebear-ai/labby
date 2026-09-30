'use client'

import { useEffect, useRef, useState } from 'react'
import type { CollectionViewMode } from '@/components/console/collection-view-toggle'

/** Follow the viewport until the operator chooses a persistent layout. */
export function useCollectionView(storageKey: string, defaultView: CollectionViewMode = 'table') {
  const [view, setView] = useState<CollectionViewMode>(defaultView)
  const chosen = useRef(false)

  useEffect(() => {
    const media = window.matchMedia?.('(max-width: 640px)')
    chosen.current = false
    try {
      const saved = window.localStorage.getItem(storageKey)
      if (saved === 'table' || saved === 'list' || saved === 'cards') {
        chosen.current = true
        setView(saved)
      }
    } catch {
      // Storage can be unavailable in private contexts; responsive defaults still work.
    }
    const applyDefault = () => {
      if (!chosen.current) setView(media?.matches ? 'cards' : defaultView)
    }
    applyDefault()
    media?.addEventListener('change', applyDefault)
    return () => media?.removeEventListener('change', applyDefault)
  }, [storageKey, defaultView])

  const selectView = (next: CollectionViewMode) => {
    chosen.current = true
    setView(next)
    try {
      window.localStorage.setItem(storageKey, next)
    } catch {
      // Keep the operator's choice for this session when persistence is unavailable.
    }
  }
  return [view, selectView] as const
}
