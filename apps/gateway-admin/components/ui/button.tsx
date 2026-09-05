import * as React from 'react'
import { Slot } from '@radix-ui/react-slot'
import { cva, type VariantProps } from 'class-variance-authority'

import { cn } from '@/lib/utils'

function readableLabel(node: React.ReactNode): string {
  if (typeof node === 'string' || typeof node === 'number') return String(node)
  if (React.isValidElement<{ children?: React.ReactNode }>(node)) {
    return readableLabel(node.props.children)
  }
  return React.Children.toArray(node).map(readableLabel).join(' ').trim()
}

/**
 * Button variants — Aurora-tuned:
 * - `default`: Aurora accent primary (action CTA)
 * - `destructive`: action-consequence only (delete, remove, irreversible)
 * - `outline`: Aurora control surface (toolbar actions)
 * - `secondary`: panel-strong background (supporting actions)
 * - `ghost`: no background, hover reveals panel (in-context actions)
 * - `link`: underline only (navigation-adjacent)
 *
 * Selected state: use `data-selected="true"` attribute.
 * This follows the existing `aria-invalid:` selector pattern (line 8).
 * Do NOT pass `selected` as a boolean prop.
 *
 * Example: <Button variant="outline" data-selected={isSelected}>
 */
const buttonVariants = cva(
  "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-all disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg:not([class*='size-'])]:size-4 shrink-0 [&_svg]:shrink-0 outline-none focus-visible:border-ring focus-visible:ring-aurora-accent-primary/34 focus-visible:ring-[3px] aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 aria-invalid:border-destructive",
  {
    variants: {
      variant: {
        default:
          'bg-primary text-primary-foreground hover:bg-aurora-accent-strong/95 data-[selected=true]:shadow-aurora-active-glow',
        destructive:
          'bg-destructive text-white hover:bg-destructive/90 focus-visible:ring-destructive/20 dark:focus-visible:ring-destructive/40 dark:bg-destructive/60',
        outline:
          'border border-aurora-border-strong bg-aurora-control-surface hover:bg-aurora-panel-medium hover:text-aurora-text-primary data-[selected=true]:shadow-aurora-active-glow data-[selected=true]:border-aurora-accent-primary/40',
        secondary:
          'bg-secondary text-secondary-foreground hover:bg-secondary/80',
        ghost:
          'hover:bg-aurora-panel-medium hover:text-aurora-text-primary dark:hover:bg-aurora-panel-medium/50',
        link: 'text-primary underline-offset-4 hover:underline',
      },
      size: {
        default: 'h-9 px-4 py-2 has-[>svg]:px-3',
        sm: 'h-8 rounded-md gap-1.5 px-3 has-[>svg]:px-2.5',
        lg: 'h-10 rounded-md px-6 has-[>svg]:px-4',
        icon: 'size-9',
        'icon-sm': 'size-8',
        'icon-lg': 'size-10',
      },
    },
    defaultVariants: {
      variant: 'default',
      size: 'default',
    },
  },
)

function Button({
  className,
  variant,
  size,
  asChild = false,
  ...props
}: React.ComponentProps<'button'> &
  VariantProps<typeof buttonVariants> & {
    asChild?: boolean
  }) {
  const Comp = asChild ? Slot : 'button'
  const buttonProps =
    asChild ? props : { type: 'button' as const, ...props }
  const label = readableLabel(props.children).replace(/\s+/g, ' ').trim()

  return (
    <Comp
      {...buttonProps}
      data-slot="button"
      aria-label={props['aria-label'] ?? (label || undefined)}
      title={props.title ?? (label || undefined)}
      className={cn(buttonVariants({ variant, size, className }))}
    />
  )
}

export { Button, buttonVariants }
