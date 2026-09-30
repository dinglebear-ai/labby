import { AppHeader } from '@/components/app-header'
import { cn } from '@/lib/utils'
import {
  AURORA_DISPLAY_1,
  AURORA_MUTED_LABEL,
  AURORA_PAGE_FRAME,
  AURORA_PAGE_SHELL,
  AURORA_STRONG_PANEL,
} from '@/components/aurora/tokens'
import { ControlsSection } from './controls-section'
import { CommandPaletteSection } from './command-palette-section'
import { DataDisplaySection } from './data-display-section'
import { FeedbackSection } from './feedback-section'
import { FoundationsSection } from './foundations-section'
import { NavigationSection } from './navigation-section'
import { PatternsSection } from './patterns-section'

export function DesignSystemShell() {
  return (
    <>
      <AppHeader
        breadcrumbs={[
          { label: 'Overview', href: '/' },
          { label: 'Design System' },
        ]}
      />
      <div className={cn(AURORA_PAGE_FRAME, AURORA_PAGE_SHELL, 'min-w-0 [&_*]:min-w-0')}>
        <div className={cn(AURORA_STRONG_PANEL, 'px-4 py-5 sm:px-6 sm:py-6')}>
          <p className={AURORA_MUTED_LABEL}>Internal Sandbox</p>
          <h1 className={cn(AURORA_DISPLAY_1, 'mt-2 text-aurora-text-primary')}>Design System</h1>
          <p className="mt-2 max-w-2xl text-sm text-aurora-text-muted sm:text-base">
            A direct-url-only playground for validating layout, typography, controls, and
            application patterns against the current admin shell without calling real backend
            services.
          </p>
        </div>

        <div className="grid gap-5 xl:grid-cols-2">
          <div className="xl:col-span-2">
            <FoundationsSection />
          </div>
          <div className="xl:col-span-2">
            <ControlsSection />
          </div>
          <div className="xl:col-span-2">
            <FeedbackSection />
          </div>
          <div className="xl:col-span-2">
            <NavigationSection />
          </div>
          <div className="xl:col-span-2">
            <CommandPaletteSection />
          </div>
          <div className="xl:col-span-2">
            <DataDisplaySection />
          </div>
          <div className="xl:col-span-2">
            <PatternsSection />
          </div>
        </div>
      </div>
    </>
  )
}
