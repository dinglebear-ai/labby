export type ArtifactFormat = 'heading' | 'bold' | 'italic' | 'inlineCode' | 'bullet' | 'numbered' | 'code'

export function formatArtifactSelection(body: string, start: number, end: number, format: ArtifactFormat) {
  const from = Math.max(0, Math.min(body.length, start))
  const to = Math.max(from, Math.min(body.length, end))
  const selected = body.slice(from, to)
  const insertion = {
    heading: `## ${selected || 'Heading'}`,
    bold: `**${selected || 'bold text'}**`,
    italic: `*${selected || 'italic text'}*`,
    inlineCode: `\`${selected || 'code'}\``,
    bullet: `- ${selected || 'item'}`,
    numbered: `1. ${selected || 'step'}`,
    code: `\`\`\`\n${selected}\n\`\`\``,
  }[format]
  return { content: body.slice(0, from) + insertion + body.slice(to), cursor: from + insertion.length }
}
