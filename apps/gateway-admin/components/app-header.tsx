'use client'

import { Fragment } from 'react'
import { createPortal } from 'react-dom'
import Link from 'next/link'
import { ChevronRight } from 'lucide-react'

import { useOptionalConsoleShell } from '@/components/console/console-shell-context'

interface AppBreadcrumb {
  label: string
  href?: string
}

interface AppHeaderProps {
  breadcrumbs?: AppBreadcrumb[]
  actions?: React.ReactNode
}

const CRUMB_RAIL_STYLE: React.CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  gap: 7,
  fontSize: 13,
  minWidth: 0,
}

function BreadcrumbTrail({ breadcrumbs }: { breadcrumbs: AppBreadcrumb[] }) {
  return (
    <>
      {breadcrumbs.map((crumb, index) => {
        const isLeaf = index === breadcrumbs.length - 1
        return (
          <Fragment key={`${crumb.label}-${index}`}>
            {index > 0 ? (
              <ChevronRight
                size={12}
                strokeWidth={1.7}
                style={{ color: 'var(--aurora-text-muted)', flexShrink: 0 }}
              />
            ) : null}
            {crumb.href && !isLeaf ? (
              <Link
                href={crumb.href}
                style={{
                  fontSize: 13,
                  fontWeight: 560,
                  color: 'var(--aurora-text-muted)',
                  whiteSpace: 'nowrap',
                  textDecoration: 'none',
                }}
              >
                {crumb.label}
              </Link>
            ) : (
              <span
                data-crumbleaf={isLeaf ? '1' : undefined}
                style={{
                  fontSize: 13,
                  fontWeight: 560,
                  color: isLeaf ? 'var(--aurora-text-primary)' : 'var(--aurora-text-muted)',
                  whiteSpace: 'nowrap',
                  overflow: 'hidden',
                  textOverflow: 'ellipsis',
                  minWidth: isLeaf && breadcrumbs.length > 1 ? 110 : undefined,
                }}
              >
                {crumb.label}
              </span>
            )}
          </Fragment>
        )
      })}
    </>
  )
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
export function AppHeader({ breadcrumbs = [], actions }: AppHeaderProps) {
  const shell = useOptionalConsoleShell()

  if (!shell) {
    return (
      <header
        data-topbar="1"
        className="flex h-14 shrink-0 items-center gap-3 border-b border-aurora-border-default/70 px-4"
      >
        <div style={CRUMB_RAIL_STYLE}>
          <BreadcrumbTrail breadcrumbs={breadcrumbs} />
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
        ? createPortal(<BreadcrumbTrail breadcrumbs={breadcrumbs} />, crumbSlot)
        : null}
      {actionSlot && actions ? createPortal(<>{actions}</>, actionSlot) : null}
    </>
  )
}
