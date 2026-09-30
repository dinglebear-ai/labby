import * as React from 'react'
import { Slot } from '@radix-ui/react-slot'
import { cva, type VariantProps } from 'class-variance-authority'

import { cn } from '@/lib/utils'

/**
 * Badge variants split into two orthogonal axes:
 *
 * `variant` — shape:
 * - `default`: filled, rounded-md
 * - `outline`: transparent background, border visible
 * - `pill`: rounded-full, slightly wider padding
 *
 * `status` — semantic tone:
 * - `default`: primary accent (call to action, neutral highlight)
 * - `warn`: amber/gold warning (operational caution)
 * - `error`: muted red error (state failure, not action)
 * - `success`: muted teal success (operational success)
 *
 * NOTE: `error` replaces `destructive` for status display.
 * `destructive` is reserved for action-consequence semantics on Button only.
 *
 * Compound examples:
 *   <Badge status="warn" />                      — filled warn pill
 *   <Badge variant="outline" status="error" />   — outlined error badge
 *   <Badge variant="pill" status="success" />    — pill-shaped success badge
 */
const badgeVariants = cva(
  'inline-flex items-center justify-center rounded-md border px-2 py-0.5 text-xs font-medium w-fit whitespace-nowrap shrink-0 [&>svg]:size-3 gap-1 [&>svg]:pointer-events-none focus-visible:border-aurora-accent-primary focus-visible:ring-aurora-accent-primary/34 focus-visible:ring-[3px] transition-[color,box-shadow] overflow-hidden',
  {
    variants: {
      /**
       * Shape axis — controls border radius, padding, and base styling.
       *
       * - `default`: filled, rounded-md (uses `status` axis for color)
       * - `outline`: transparent background, border visible (uses `status` axis for color)
       * - `pill`: rounded-full, slightly wider padding (uses `status` axis for color)
       * - `secondary`: neutral muted surface, ignores `status` axis
       */
      variant: {
        default: '',
        outline: 'bg-transparent',
        pill: 'rounded-full px-2.5',
        secondary: 'border-transparent bg-secondary text-secondary-foreground [a&]:hover:bg-secondary/90',
      },
      /**
       * Tone axis — controls color semantics.
       * Ignored when `variant="secondary"` (secondary provides its own complete styling).
       */
      status: {
        default: '',
        warn: '',
        error: '',
        success: '',
      },
    },
    compoundVariants: [
      // default variant × status — color fills
      { variant: 'default', status: 'default', class: 'border-transparent bg-primary text-primary-foreground [a&]:hover:bg-primary/90' },
      { variant: 'default', status: 'warn', class: 'bg-aurora-warn/12 border-aurora-warn/30 text-aurora-warn' },
      { variant: 'default', status: 'error', class: 'bg-aurora-error/12 border-aurora-error/30 text-aurora-error' },
      { variant: 'default', status: 'success', class: 'bg-aurora-success/12 border-aurora-success/30 text-aurora-success' },
      // outline variant × status — border + text color only
      { variant: 'outline', status: 'default', class: 'border-border text-foreground [a&]:hover:bg-aurora-hover-bg [a&]:hover:text-aurora-text-primary' },
      { variant: 'outline', status: 'warn', class: 'border-aurora-warn/40 text-aurora-warn' },
      { variant: 'outline', status: 'error', class: 'border-aurora-error/40 text-aurora-error' },
      { variant: 'outline', status: 'success', class: 'border-aurora-success/40 text-aurora-success' },
      // pill variant × status — same as default (rounded-full shape, color from status)
      { variant: 'pill', status: 'default', class: 'border-transparent bg-primary text-primary-foreground [a&]:hover:bg-primary/90' },
      { variant: 'pill', status: 'warn', class: 'bg-aurora-warn/12 border-aurora-warn/30 text-aurora-warn' },
      { variant: 'pill', status: 'error', class: 'bg-aurora-error/12 border-aurora-error/30 text-aurora-error' },
      { variant: 'pill', status: 'success', class: 'bg-aurora-success/12 border-aurora-success/30 text-aurora-success' },
    ],
    defaultVariants: {
      variant: 'default',
      status: 'default',
    },
  },
)

function Badge({
  className,
  variant,
  status,
  asChild = false,
  ...props
}: React.ComponentProps<'span'> &
  VariantProps<typeof badgeVariants> & { asChild?: boolean }) {
  const Comp = asChild ? Slot : 'span'

  return (
    <Comp
      data-slot="badge"
      className={cn(badgeVariants({ variant, status }), className)}
      {...props}
    />
  )
}

export { Badge, badgeVariants }
