'use client'

import { CircleAlert, CircleCheck } from 'lucide-react'
import type { ArtifactIssue, ArtifactKind, ArtifactMetadata } from '@/lib/editor/artifact-standards'
import {
  ARTIFACT_VALIDATION_FIELDS as CHECKS,
  artifactValidationSummary,
  skillAuthoringSummary,
} from '@/lib/editor/artifact-validation-summary'

export function ArtifactValidationPanel({ kind, metadata, content, issues, onField }: {
  kind: ArtifactKind
  metadata: ArtifactMetadata
  content: string
  issues: ArtifactIssue[]
  onField: (field: ArtifactIssue['field']) => void
}) {
  const skillSummary = kind === 'Skill' ? skillAuthoringSummary(metadata, content) : null
  const genericSummary = artifactValidationSummary(issues)
  const rows = skillSummary ? skillSummary.checks : CHECKS.map(([field, label]) => {
    const fieldIssues = issues.filter(issue => issue.field === field)
    return {
      id: field,
      field,
      label,
      description: fieldIssues.length ? fieldIssues.map(issue => issue.message).join(' ') : 'No validation issues.',
      passing: fieldIssues.length === 0,
      optional: false,
    }
  })
  const passing = skillSummary?.passing ?? genericSummary.passing
  const total = skillSummary?.total ?? genericSummary.total

  return <aside aria-label="Draft validation" className="overflow-hidden rounded-aurora-2 border border-aurora-border-default bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] shadow-aurora-medium">
    <div className="border-b border-aurora-border-default bg-aurora-page-bg/35 px-[15px] py-[14px]">
      <div className="flex items-center justify-between gap-2 text-[9.5px] font-bold uppercase tracking-[.13em] text-aurora-text-muted"><h2>Validation</h2><span>{passing} of {total}</span></div>
      <div role="progressbar" aria-label="Passing validation checks" aria-valuemin={0} aria-valuemax={total} aria-valuenow={passing} className="mt-2 h-[3px] overflow-hidden rounded-full bg-aurora-border-subtle"><div className={passing === total ? 'h-full bg-aurora-success' : 'h-full bg-aurora-warn'} style={{ width: `${passing / total * 100}%` }} /></div>
    </div>
    {rows.map(check => {
      const fieldIssues = issues.filter(issue => issue.field === check.field)
      const tone = check.passing ? 'text-aurora-success' : fieldIssues.some(issue => issue.severity === 'error') ? 'text-aurora-error' : 'text-aurora-warn'
      return <button key={check.id} type="button" onClick={() => onField(check.field)} className="flex w-full items-start gap-[9px] border-t border-aurora-border-subtle px-[15px] py-[9px] text-left hover:bg-aurora-hover-bg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-aurora-accent-primary">
        <span className={`mt-px shrink-0 ${tone}`}>{check.passing ? <CircleCheck className="size-[13px]" /> : <CircleAlert className="size-[13px]" />}</span>
        <span className="flex min-w-0 flex-col gap-0.5"><span className="text-[11.5px] font-[650] leading-[14px] text-aurora-text-primary">{check.label}</span><span className="text-[10.5px] leading-[1.45] text-aurora-text-muted">{check.description}</span></span>
      </button>
    })}
  </aside>
}
