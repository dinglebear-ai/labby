'use client'

import { useEffect, useMemo, useRef, useState } from 'react'

import { Button } from '@/components/ui/button'
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandList,
} from '@/components/ui/command'
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from '@/components/ui/empty'
import { Kbd, KbdGroup } from '@/components/ui/kbd'
import { cn } from '@/lib/utils'
import {
  AURORA_MUTED_LABEL,
  controlTone,
} from '@/components/aurora/tokens'
import type { CommandPaletteItem } from './command-palette-data'
import { CommandPalettePreview } from './command-palette-preview'
import { CommandPaletteRow } from './command-palette-row'
import {
  buildCommandPaletteState,
  findItemById,
  getNextActiveItemId,
} from './command-palette-model'

export function CommandPaletteDemo() {
  const initialState = buildCommandPaletteState('')
  const [open, setOpen] = useState(true)
  const [query, setQuery] = useState('')
  const [activeItemId, setActiveItemId] = useState<string | null>(initialState.activeItemId)
  const [lastActionLabel, setLastActionLabel] = useState(
    'Waiting for a selection. Use the keyboard or pointer to simulate a command.',
  )
  const inputRef = useRef<HTMLInputElement>(null)

  const state = useMemo(() => buildCommandPaletteState(query), [query])

  useEffect(() => {
    setActiveItemId((current) => {
      if (current && state.items.some((item) => item.id === current)) return current
      return state.activeItemId
    })
  }, [state.activeItemId, state.items])

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
        event.preventDefault()
        setOpen(true)
      }
    }

    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [])

  useEffect(() => {
    if (!open) return

    const frame = window.requestAnimationFrame(() => {
      inputRef.current?.focus()
    })

    return () => window.cancelAnimationFrame(frame)
  }, [open])

  const activeItem = findItemById(activeItemId, state.items)

  function commitSelection(item: CommandPaletteItem | null) {
    if (!item) return
    setLastActionLabel(`${item.actionHint}: ${item.title}`)
    setOpen(false)
  }

  function handleListKeyDown(event: React.KeyboardEvent) {
    if (event.key === 'ArrowDown') {
      event.preventDefault()
      setActiveItemId((current) => getNextActiveItemId(current, state.items, 'next'))
      return
    }

    if (event.key === 'ArrowUp') {
      event.preventDefault()
      setActiveItemId((current) => getNextActiveItemId(current, state.items, 'previous'))
      return
    }

    if (event.key === 'Enter') {
      event.preventDefault()
      commitSelection(activeItem)
      return
    }

    if (event.key === 'Escape') {
      event.preventDefault()
      setOpen(false)
    }
  }

  return (
    <div className="grid gap-4 xl:grid-cols-[minmax(0,1.12fr)_minmax(320px,0.88fr)]">
      <div className="grid gap-4">
        <div className="flex flex-wrap items-center justify-between gap-3 rounded-aurora-2 border border-aurora-border-strong bg-aurora-panel-medium px-4 py-4">
          <div>
            <p className={AURORA_MUTED_LABEL}>Interactive prototype</p>
            <div className="mt-2 max-w-2xl text-sm text-aurora-text-muted">
              Open by default in the sandbox. Reopen with the button or press
              <span className="mx-1 inline-flex">
                <KbdGroup>
                  <Kbd className="border border-aurora-border-strong bg-aurora-control-surface text-aurora-text-muted">
                    ⌘
                  </Kbd>
                  <Kbd className="border border-aurora-border-strong bg-aurora-control-surface text-aurora-text-muted">
                    K
                  </Kbd>
                </KbdGroup>
              </span>
              / `Ctrl+K`.
            </div>
          </div>
          <Button
            type="button"
            variant="outline"
            className={cn(controlTone(), 'rounded-aurora-1 hover:bg-aurora-hover-bg hover:text-aurora-text-primary')}
            onClick={() => setOpen(true)}
          >
            Reopen palette
          </Button>
        </div>

        {open ? (
          <div className="rounded-aurora-3 border border-aurora-border-strong bg-aurora-panel-strong shadow-[var(--aurora-shadow-strong),var(--aurora-highlight-strong)]">
            <Command
              shouldFilter={false}
              onKeyDownCapture={handleListKeyDown}
              className="bg-transparent text-aurora-text-primary"
            >
              <div className="border-b border-aurora-border-strong px-4 py-3">
                <div className="mb-3 flex flex-wrap items-center justify-between gap-3">
                  <div>
                    <p className={AURORA_MUTED_LABEL}>Global command palette</p>
                    <p className="mt-2 text-sm text-aurora-text-muted">
                      Search pages, commands, and recent context in one ranked list.
                    </p>
                  </div>
                  <div className="flex items-center gap-2 text-aurora-text-muted">
                    <Kbd className="border border-aurora-border-strong bg-aurora-control-surface text-aurora-text-muted">
                      ↑↓
                    </Kbd>
                    <Kbd className="border border-aurora-border-strong bg-aurora-control-surface text-aurora-text-muted">
                      Enter
                    </Kbd>
                    <Kbd className="border border-aurora-border-strong bg-aurora-control-surface text-aurora-text-muted">
                      Esc
                    </Kbd>
                  </div>
                </div>
                <CommandInput
                  ref={inputRef}
                  value={query}
                  onValueChange={setQuery}
                  aria-label="Search command palette"
                  name="design-system-command-palette-search"
                  placeholder="Search pages, commands, and recent context..."
                  className="text-aurora-text-primary placeholder:text-aurora-text-muted"
                />
              </div>

              <CommandList className="aurora-scrollbar max-h-[390px] px-4 py-4">
                {state.items.length ? (
                  state.groups.map((group) => (
                    <CommandGroup
                      key={group.key}
                      heading={group.label}
                      className="mb-3 overflow-visible p-0 [&_[cmdk-group-heading]]:px-0 [&_[cmdk-group-heading]]:py-0 [&_[cmdk-group-heading]]:pb-2 [&_[cmdk-group-heading]]:text-[11px] [&_[cmdk-group-heading]]:font-bold [&_[cmdk-group-heading]]:tracking-[0.18em] [&_[cmdk-group-heading]]:text-aurora-text-muted [&_[cmdk-group-heading]]:uppercase"
                    >
                      <div className="grid gap-2">
                        {group.items.map((item) => (
                          <CommandPaletteRow
                            key={item.id}
                            item={item}
                            active={item.id === activeItemId}
                            onActivate={setActiveItemId}
                            onSelect={(itemId) => {
                              setActiveItemId(itemId)
                              commitSelection(findItemById(itemId, state.items))
                            }}
                          />
                        ))}
                      </div>
                    </CommandGroup>
                  ))
                ) : (
                  <CommandEmpty className="py-0">
                    <Empty className="rounded-aurora-2 border-aurora-border-strong bg-aurora-control-surface p-6">
                      <EmptyHeader className="items-start text-left">
                        <EmptyTitle className="text-[15px] font-semibold text-aurora-text-primary">
                          No matching commands
                        </EmptyTitle>
                        <EmptyDescription className="text-left text-aurora-text-muted">
                          Try a broader term like gateway, logs, settings, or token to bring mixed results back into the stack.
                        </EmptyDescription>
                      </EmptyHeader>
                    </Empty>
                  </CommandEmpty>
                )}
              </CommandList>
            </Command>
          </div>
        ) : (
          <div className="rounded-aurora-3 border border-dashed border-aurora-border-strong bg-aurora-control-surface px-4 py-5 text-sm text-aurora-text-muted">
            Palette closed. Reopen it with the button above or `Cmd/Ctrl+K`.
          </div>
        )}
      </div>

      <CommandPalettePreview item={activeItem} lastActionLabel={lastActionLabel} />
    </div>
  )
}
