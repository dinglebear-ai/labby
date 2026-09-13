'use client'

import { Fragment } from 'react'
import { createPortal } from 'react-dom'
import Link from 'next/link'
import { ChevronRight } from 'lucide-react'
import { consoleNavItems } from '@/components/console/nav-model'

import { useOptionalConsoleShell } from '@/components/console/console-shell-context'

interface AppBreadcrumb {
  label: string
  href?: string
}

interface AppHeaderProps {
  breadcrumbs?: AppBreadcrumb[]
  actions?: React.ReactNode
  icon?: React.ReactNode
}

const CRUMB_RAIL_STYLE: React.CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  gap: 3,
  fontSize: 12.5,
  lineHeight: 'normal',
  minWidth: 0,
}

function BreadcrumbTrail({ breadcrumbs, icon }: { breadcrumbs: AppBreadcrumb[]; icon?: React.ReactNode }) {
  return <>{breadcrumbs.map((crumb, index) => {
    const isLeaf = index === breadcrumbs.length - 1
    const Icon = consoleNavItems.find(item => item.label === crumb.label)?.icon
    const mark = index === 0 && icon ? icon : Icon ? <Icon size={14} strokeWidth={1.7} /> : null
    const style: React.CSSProperties = {
      display: 'inline-flex', alignItems: 'center', gap: 6, height: 26,
      padding: index > 0 ? '0 10px' : '0 9px', borderRadius: 8,
      border: index > 0 ? '1px solid color-mix(in srgb, var(--aurora-accent-primary) 26%, transparent)' : '1px solid transparent',
      background: index > 0 ? 'color-mix(in srgb, var(--aurora-accent-primary) 8%, transparent)' : 'none',
      fontSize: 12.5, lineHeight: 'normal', fontWeight: index > 0 ? 700 : 600,
      color: isLeaf ? 'var(--aurora-text-primary)' : 'var(--aurora-text-muted)',
      whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis',
      minWidth: index > 0 ? 96 : undefined, maxWidth: 220, textDecoration: 'none',
    }
    const content = <>{mark ? <span aria-hidden className="grid shrink-0 place-items-center text-aurora-accent-strong">{mark}</span> : null}<span className="truncate">{crumb.label}</span></>
    return <Fragment key={`${crumb.label}-${index}`}>
      {index > 0 ? <ChevronRight size={13} strokeWidth={1.7} className="shrink-0 text-aurora-text-muted" /> : null}
      {crumb.href && !isLeaf ? <Link href={crumb.href} style={style}>{content}</Link> : <span data-crumbleaf={isLeaf ? '1' : undefined} style={style}>{content}</span>}
    </Fragment>
  })}</>
}

/**
 * Per-screen contribution to the console's single topbar.
 *
 * Inside the console shell there is exactly one `<header>`, so a screen
 * declares only its breadcrumb trail and action cluster and those are
 * portalled into the slots the shell registered. Rendered outside the shell
 * — unit tests, isolated stories, the standalone Code Mode route — it falls
 * back to an inline header so the same content still appears.
 */
export function AppHeader({ breadcrumbs = [], actions, icon }: AppHeaderProps) {
  const shell = useOptionalConsoleShell()
  const trail = <BreadcrumbTrail breadcrumbs={breadcrumbs} icon={icon} />

  if (!shell) {
    return (
      <header
        data-topbar="1"
        className="flex h-14 shrink-0 items-center gap-3 border-b border-aurora-border-default/70 px-4"
      >
        <div style={CRUMB_RAIL_STYLE}>
          {trail}
        </div>
        <div style={{ flex: 1 }} />
        <div data-actioncluster="1" className="flex shrink-0 items-center gap-1.5">
          {actions}
        </div>
      </header>
    )
  }

  const { crumbSlot, actionSlot } = shell

  return (
    <>
      {crumbSlot
        ? createPortal(trail, crumbSlot)
        : null}
      {actionSlot && actions ? createPortal(<>{actions}</>, actionSlot) : null}
    </>
  )
}
