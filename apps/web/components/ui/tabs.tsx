'use client'

import * as React from 'react'
import * as TabsPrimitive from '@radix-ui/react-tabs'

import { cn } from '@/lib/utils'

/**
 * Tabs — Aurora restrained active state:
 * - Active tab: border-bottom indicator + accent text + subtle glow
 * - No filled pill background on active state (contract spec)
 */

function Tabs({
  className,
  ...props
}: React.ComponentProps<typeof TabsPrimitive.Root>) {
  return (
    <TabsPrimitive.Root
      data-slot="tabs"
      className={cn('flex flex-col gap-2', className)}
      {...props}
    />
  )
}

function TabsList({
  className,
  ...props
}: React.ComponentProps<typeof TabsPrimitive.List>) {
  return (
    <TabsPrimitive.List
      data-slot="tabs-list"
      className={cn(
        'bg-aurora-control-surface text-aurora-text-muted inline-flex h-9 max-w-full items-center justify-start overflow-x-auto rounded-lg p-[3px] md:w-fit md:justify-center',
        className,
      )}
      {...props}
    />
  )
}

function TabsTrigger({
  className,
  ...props
}: React.ComponentProps<typeof TabsPrimitive.Trigger>) {
  return (
    <TabsPrimitive.Trigger
      data-slot="tabs-trigger"
      className={cn(
        // Base
        "inline-flex h-[calc(100%-1px)] shrink-0 items-center justify-center gap-1.5 rounded-md border border-transparent px-2 py-1 text-sm font-medium whitespace-nowrap transition-[color,box-shadow]",
        // Focus
        "focus-visible:border-aurora-accent-primary focus-visible:ring-aurora-accent-primary/34 focus-visible:ring-[3px] focus-visible:outline-1",
        // Disabled
        "disabled:pointer-events-none disabled:opacity-50",
        // SVG
        "[&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
        // Responsive
        "md:flex-1",
        // Default (inactive) state
        "text-foreground dark:text-muted-foreground",
        // Active state — border indicator + accent text, NO filled background
        "data-[state=active]:border-b-2 data-[state=active]:border-aurora-accent-primary data-[state=active]:text-aurora-text-primary data-[state=active]:shadow-aurora-active-glow",
        className,
      )}
      {...props}
    />
  )
}

function TabsContent({
  className,
  ...props
}: React.ComponentProps<typeof TabsPrimitive.Content>) {
  return (
    <TabsPrimitive.Content
      data-slot="tabs-content"
      className={cn('flex-1 outline-none', className)}
      {...props}
    />
  )
}

export { Tabs, TabsList, TabsTrigger, TabsContent }
