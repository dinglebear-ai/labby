import { normalizeArtifactTags, type ArtifactIssue, type ArtifactMetadata } from './artifact-standards'

// Every field validateArtifactDraft can report is listed, so no issue is dropped from the pass count.
export const ARTIFACT_VALIDATION_FIELDS = [
  ['name', 'Name'], ['description', 'Description'], ['tags', 'Tags'], ['content', 'Body'],
  ['license', 'License'], ['compatibility', 'Compatibility'], ['allowedTools', 'Allowed tools'],
] as const satisfies ReadonlyArray<readonly [ArtifactIssue['field'], string]>

export function artifactValidationSummary(issues: ArtifactIssue[]) {
  const total = ARTIFACT_VALIDATION_FIELDS.length
  const passing = ARTIFACT_VALIDATION_FIELDS.filter(([field]) => !issues.some(issue => issue.field === field)).length
  return { total, passing }
}

export interface ArtifactAuthoringCheck {
  id: 'name' | 'description' | 'tags' | 'sections' | 'substance' | 'example'
  field: keyof ArtifactMetadata | 'content'
  label: string
  description: string
  passing: boolean
  optional?: boolean
}

/**
 * Skill authoring checks. Validator errors for a field (`issues`) override a
 * heuristic pass so a blocking problem such as a tag over Depot's limit is
 * visible in the same list instead of only disabling Publish.
 */
export function skillAuthoringChecks(metadata: ArtifactMetadata, content: string, issues: readonly ArtifactIssue[] = []): ArtifactAuthoringCheck[] {
  const name = metadata.name.trim()
  const description = metadata.description.trim()
  const tags = normalizeArtifactTags(metadata.tags)
  const body = content.trim()
  const hasWhenToUse = /^##\s+When to use\b/im.test(content)
  const hasSteps = /^##\s+(Steps|Workflow|Instructions)\b/im.test(content)
  const hasExample = /^##\s+Examples?\b/im.test(content) || /\bworked example\b/i.test(content)

  const checks: ArtifactAuthoringCheck[] = [
    {
      id: 'name', field: 'name', label: 'Name is a slug',
      description: 'Lowercase, hyphenated, no spaces — harnesses key on it.',
      passing: Boolean(name) && name.length <= 64 && /^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(name),
    },
    {
      id: 'description', field: 'description', label: 'Description is loadable',
      description: 'One line, under 220 characters. This is what the model reads to decide.',
      passing: Boolean(description) && description.length <= 220 && !/[\r\n]/.test(description),
    },
    {
      id: 'tags', field: 'tags', label: 'At least two tags',
      description: 'Tags scope team access and drive Bazaar filtering.',
      passing: tags.length >= 2,
    },
    {
      id: 'sections', field: 'content', label: 'Body has sections',
      description: 'A When-to-use section plus concrete steps beats a wall of prose.',
      passing: hasWhenToUse && hasSteps,
    },
    {
      id: 'substance', field: 'content', label: 'Body has substance',
      description: 'Under ~160 characters the artifact rarely changes behavior.',
      passing: body.length >= 160,
    },
    {
      id: 'example', field: 'content', label: 'Has a worked example',
      description: 'Optional — adding one worked example measurably improves adherence.',
      passing: hasExample,
      optional: true,
    },
  ]
  return checks.map(check => {
    if (!check.passing) return check
    const blocking = issues.filter(issue => issue.field === check.field && issue.severity === 'error')
    return blocking.length ? { ...check, passing: false, description: blocking.map(issue => issue.message).join(' ') } : check
  })
}

export function skillAuthoringSummary(metadata: ArtifactMetadata, content: string, issues: readonly ArtifactIssue[] = []) {
  const checks = skillAuthoringChecks(metadata, content, issues)
  return { checks, passing: checks.filter(check => check.passing).length, total: checks.length }
}
