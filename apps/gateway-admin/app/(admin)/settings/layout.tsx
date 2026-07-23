import type { ReactNode } from 'react'
import Link from 'next/link'
import { Power } from 'lucide-react'

import { SettingsRail } from '@/components/settings/SettingsRail'
import { DraftStaleBanner } from '@/components/settings/DraftStaleBanner'

export default function SettingsLayout({
  children,
}: {
  children: ReactNode
}): React.ReactElement {
  return (
    <div
      data-unraid-settings-shell
      className="unraid-settings-shell fixed inset-0 z-40 min-h-screen overflow-y-auto bg-[#f2f2f2] font-sans text-[#1c1b1b]"
    >
      <header className="sticky top-0 z-30 border-b border-[#e5e5e5] bg-white shadow-[0_1px_2px_rgba(0,0,0,0.04)]">
        <div
          aria-hidden="true"
          className="h-[3px]"
          style={{ background: 'linear-gradient(90deg,#e22828,#ff8c2f)' }}
        />
        <div className="mx-auto flex h-[60px] max-w-[1440px] items-center gap-3 px-5">
          <Link href="/" className="flex min-w-0 items-center gap-2.5 no-underline">
            <span className="grid size-8 place-items-center rounded-md bg-[#1c1b1b] text-[#ff8c2f]">
              <Power aria-hidden="true" className="size-[19px]" strokeWidth={2.4} />
            </span>
            <span className="whitespace-nowrap text-[17px] font-semibold text-[#1c1b1b]">Labby</span>
            <span className="hidden items-center gap-1.5 rounded-full bg-[#dcfce7] px-2.5 py-1 text-xs font-semibold text-[#457b3e] sm:inline-flex">
              <span className="size-[7px] rounded-full bg-current" />
              Configured
            </span>
          </Link>
          <div className="flex-1" />
          <nav
            aria-label="Primary"
            className="hidden items-center justify-center gap-0.5 rounded-md bg-[#e5e5e5] p-[5px] sm:inline-flex"
          >
            <Link href="/" className="rounded-md px-3.5 py-1.5 text-sm font-medium text-[#1c1b1b] no-underline hover:bg-white">
              Overview
            </Link>
            <Link href="/gateways" className="rounded-md px-3.5 py-1.5 text-sm font-medium text-[#1c1b1b] no-underline hover:bg-white">
              Gateway
            </Link>
            <Link
              href="/settings/core/"
              aria-current="page"
              className="rounded-md bg-[linear-gradient(90deg,#e22828,#ff8c2f)] px-3.5 py-1.5 text-sm font-medium text-white no-underline shadow-sm"
            >
              Settings
            </Link>
          </nav>
          <span className="hidden whitespace-nowrap px-2 font-mono text-xs text-[#737373] lg:inline">
            mcp.dinglebear.ai
          </span>
          <span className="hidden whitespace-nowrap font-mono text-xs text-[#a3a3a3] xl:inline">
            v1.6.0
          </span>
        </div>
      </header>

      <main className="mx-auto flex w-full max-w-[1440px] flex-col gap-4 p-5">
        <section className="overflow-hidden rounded-[6px] border-2 border-[#f5f5f5] bg-white shadow-[0_4px_6px_-1px_rgba(0,0,0,0.08)]">
          <div className="flex flex-wrap items-end justify-between gap-4 px-6 pb-4 pt-5">
            <div>
              <p className="text-[11px] font-semibold uppercase tracking-[0.1em] text-[#737373]">
                Gateway Control Plane
              </p>
              <h1 className="mt-1.5 text-[26px] font-semibold leading-none tracking-[-0.02em]">
                Settings
              </h1>
            </div>
            <p className="max-w-xl text-right text-xs leading-5 text-[#737373]">
              Configure the running Labby gateway. Writes are backup-first and stale-value protected.
            </p>
          </div>
          <div className="border-t border-[#f0f0f0] bg-[#fafafa]">
            <aside className="min-w-0">
              <SettingsRail />
            </aside>
          </div>
        </section>

        <DraftStaleBanner />
        <section className="min-w-0">{children}</section>
      </main>
    </div>
  )
}
