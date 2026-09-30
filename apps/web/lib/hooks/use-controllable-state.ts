'use client'

import { useCallback, useEffect, useRef, useState } from 'react'

interface UseControllableStateOptions<T> {
  prop: T | undefined
  defaultProp: T | undefined
  onChange?: (value: T) => void
}

export function useControllableState<T>({
  prop,
  defaultProp,
  onChange,
}: UseControllableStateOptions<T>): [T | undefined, (next: T | ((prev: T | undefined) => T)) => void] {
  const [uncontrolled, setUncontrolled] = useState<T | undefined>(defaultProp)
  const isControlled = prop !== undefined
  const value = isControlled ? prop : uncontrolled

  const onChangeRef = useRef(onChange)
  useEffect(() => {
    onChangeRef.current = onChange
  }, [onChange])

  const isControlledRef = useRef(isControlled)
  const propRef = useRef(prop)
  useEffect(() => {
    isControlledRef.current = isControlled
    propRef.current = prop
  }, [isControlled, prop])

  const setValue = useCallback((next: T | ((prev: T | undefined) => T)) => {
    const isFn = typeof next === 'function'
    if (isControlledRef.current) {
      const resolved = isFn
        ? (next as (prev: T | undefined) => T)(propRef.current)
        : next
      if (resolved !== propRef.current) {
        onChangeRef.current?.(resolved)
      }
      return
    }
    setUncontrolled((prev) => {
      const resolved = isFn ? (next as (prev: T | undefined) => T)(prev) : next
      if (resolved !== prev) {
        onChangeRef.current?.(resolved)
      }
      return resolved
    })
  }, [])

  return [value, setValue]
}
