import * as React from 'react'
import { cva, type VariantProps } from 'class-variance-authority'

import { cn } from '@/lib/utils'

/**
 * Card variants:
 * - `medium` (default): Tier 2 surface — standard content panels.
 * - `strong`: Tier 3 surface — elevated/featured panels with stronger shadow.
 *
 * The `variant` axis mirrors Button/Badge API convention (not a `tier` prop).
 * Tier naming maps to: medium = Tier 2, strong = Tier 3.
 */
const cardVariants = cva(
  'flex flex-col gap-6 rounded-aurora-3 border bg-card text-card-foreground',
  {
    variants: {
      variant: {
        /** Default: Tier 2 medium panel — most content cards. */
        medium:
          'border-aurora-border-strong bg-aurora-panel-medium shadow-aurora-medium shadow-aurora-highlight-medium',
        /** Tier 3 strong panel — elevated/featured sections. */
        strong:
          'border-aurora-border-strong bg-aurora-panel-strong rounded-aurora-3 shadow-aurora-strong shadow-aurora-highlight-strong',
      },
    },
    defaultVariants: {
      variant: 'medium',
    },
  },
)

function Card({
  className,
  variant,
  ...props
}: React.ComponentProps<'div'> & VariantProps<typeof cardVariants>) {
  const resolvedVariant = variant ?? 'medium'
  return (
    <div
      data-slot="card"
      data-variant={resolvedVariant}
      className={cn(cardVariants({ variant: resolvedVariant }), className)}
      {...props}
    />
  )
}

function CardHeader({ className, ...props }: React.ComponentProps<'div'>) {
  return (
    <div
      data-slot="card-header"
      className={cn(
        '@container/card-header grid auto-rows-min grid-rows-[auto_auto] items-start gap-2 px-6 pt-6 has-data-[slot=card-action]:grid-cols-[1fr_auto] [&.border-b]:pb-6',
        className,
      )}
      {...props}
    />
  )
}

function CardTitle({ className, ...props }: React.ComponentProps<'div'>) {
  return (
    <div
      data-slot="card-title"
      className={cn(
        'font-display leading-none font-semibold tracking-[-0.02em]',
        className,
      )}
      {...props}
    />
  )
}

function CardDescription({ className, ...props }: React.ComponentProps<'div'>) {
  return (
    <div
      data-slot="card-description"
      className={cn('text-muted-foreground text-sm', className)}
      {...props}
    />
  )
}

function CardAction({ className, ...props }: React.ComponentProps<'div'>) {
  return (
    <div
      data-slot="card-action"
      className={cn(
        'col-start-2 row-span-2 row-start-1 self-start justify-self-end',
        className,
      )}
      {...props}
    />
  )
}

function CardContent({ className, ...props }: React.ComponentProps<'div'>) {
  return (
    <div
      data-slot="card-content"
      className={cn('px-6', className)}
      {...props}
    />
  )
}

function CardFooter({ className, ...props }: React.ComponentProps<'div'>) {
  return (
    <div
      data-slot="card-footer"
      className={cn('flex items-center px-6 pb-6 [&.border-t]:pt-6', className)}
      {...props}
    />
  )
}

export {
  Card,
  CardHeader,
  CardFooter,
  CardTitle,
  CardAction,
  CardDescription,
  CardContent,
  cardVariants,
}
