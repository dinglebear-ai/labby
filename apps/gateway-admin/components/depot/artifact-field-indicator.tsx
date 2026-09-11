import type { ArtifactIssue } from '@/lib/editor/artifact-standards'

export function ArtifactFieldIndicator({ field, issues, className = '' }: { field: ArtifactIssue['field']; issues: ArtifactIssue[]; className?: string }) {
  const matches = issues.filter(issue => issue.field === field)
  const tone = matches.some(issue => issue.severity === 'error') ? 'var(--aurora-error)' : matches.length ? 'var(--aurora-warn)' : 'var(--aurora-success)'
  const message = matches.length ? matches.map(issue => issue.message).join(' ') : 'No validation issues.'
  return <span role="img" aria-label={`${field}: ${message}`} title={message} className={`inline-block size-1.5 shrink-0 rounded-full ${className}`} style={{ background: tone, boxShadow: `0 0 4px ${tone}` }} />
}
