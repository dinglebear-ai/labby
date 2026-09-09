import { ChevronLeft, ChevronRight, Container } from 'lucide-react'
import { Button } from '@/components/ui/button'

export function ContainerWizardFooter({ step, summary, onStep }: { step: number; summary: string; onStep: (step: number) => void }) {
  return <footer className="flex shrink-0 flex-wrap items-center gap-[9px] border-t border-aurora-border-subtle bg-[var(--gw0-0_38)] px-4 py-3">
    <span className="min-w-0 flex-1 basis-full text-[11px] text-aurora-text-muted sm:basis-auto">{step < 6 ? summary : 'Container creation is not connected to a runtime yet.'}</span>
    <div className="ml-auto flex shrink-0 gap-2">
      <Button variant="outline" data-visible-label="1" className="h-8 rounded-[9px] px-3.5 text-xs" disabled={step === 0} onClick={() => onStep(step - 1)}><ChevronLeft className="size-3.5"/>Back</Button>
      {step < 6 ? <Button variant="outline" data-visible-label="1" className="h-8 rounded-[9px] px-3.5 text-xs" onClick={() => onStep(step + 1)}>Next<ChevronRight className="size-3.5"/></Button> : <Button data-visible-label="1" disabled title="Container creation is unavailable"><Container/>Build unavailable</Button>}
    </div>
  </footer>
}
