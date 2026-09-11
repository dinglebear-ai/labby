import type { ArtifactKind } from '@/lib/editor/artifact-standards'

const EXAMPLES: Partial<Record<ArtifactKind, string>> = {
  Skill: 'name: repo-triage\ndescription: Cluster open PRs by\n  subsystem and draft a note each.\n\n## When to use\nInvoke when asked to triage,\ngroup or summarize open work.',
  Agent: 'name: reviewer\ndescription: Review a proposed change.\n\n## Instructions\nInspect the diff and report\nactionable findings with evidence.',
  Command: 'name: review\ndescription: Review the current diff.\n\n## Steps\n1. Inspect the changed files.\n2. Summarize concrete findings.',
  Prompt: 'name: summarize\ndescription: Summarize supplied text.\n\n## Instructions\nReturn the main points and\nidentify unresolved questions.',
  Hook: '#!/bin/sh\n# Example hook body\nprintf \'%s\\n\' "Review the pending change"',
}

export function ArtifactWritingExample({ kind }: { kind: ArtifactKind }) {
  return <section aria-label={`Writing example for ${kind}`} className="mt-3 overflow-hidden rounded-aurora-2 border border-aurora-border-default bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] shadow-aurora-medium">
    <h2 className="border-b border-aurora-border-default bg-aurora-page-bg/35 px-[15px] py-2.5 text-[9.5px] font-bold uppercase tracking-[.13em] text-aurora-text-muted">Writing {kind === 'Agent' ? 'an' : 'a'} {kind.toLowerCase()}</h2>
    <pre className="m-0 whitespace-pre-wrap break-words px-[15px] py-3 font-mono text-[10.5px] leading-[1.6] text-aurora-text-muted">{EXAMPLES[kind] ?? 'Use a JSON object matching the\nintended consumer’s schema.\n\nNo canonical example is supplied\nfor this artifact type.'}</pre>
    <p className="border-t border-aurora-border-subtle px-[15px] py-2 text-[10.5px] leading-[1.45] text-aurora-text-muted">Illustrative example — not part of your draft.</p>
  </section>
}
