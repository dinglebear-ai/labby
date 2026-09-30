import type { ArtifactMetadata } from '@/lib/editor/artifact-standards'
import { gatewayDisplayName } from '@/lib/gateway-display-name'
import type { Gateway } from '@/lib/types/gateway'

export interface GatewaySkillDraft {
  metadata: ArtifactMetadata
  content: string
}

function skillSlug(identifier: string): string {
  const normalized = identifier.trim().toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').replace(/-{2,}/g, '-')
  return normalized || 'gateway-server'
}

function bullet(name: string, description?: string): string {
  return description?.trim() ? '- `' + name + '` — ' + description.trim() : '- `' + name + '`'
}

/** Build a truthful starter Skill from the discovery data the gateway actually reports. */
export function gatewaySkillDraft(gateway: Gateway): GatewaySkillDraft {
  const displayName = gatewayDisplayName(gateway.name)
  const exposedTools = gateway.discovery.tools.filter((tool) => tool.exposed)
  const resources = gateway.discovery.resources.filter((resource) => resource.exposed !== false)
  const prompts = gateway.discovery.prompts.filter((prompt) => prompt.exposed !== false)
  const slug = skillSlug(gateway.name)
  const toolLines = exposedTools.length ? exposedTools.map((tool) => bullet(tool.name, tool.description)).join('\n') : '- No exposed tools are currently reported by this server.'
  const resourceLines = resources.length ? resources.map((resource) => bullet(resource.name || resource.uri, resource.description)).join('\n') : '- No exposed resources are currently reported by this server.'
  const promptLines = prompts.length ? prompts.map((prompt) => bullet(prompt.name, prompt.description)).join('\n') : '- No exposed prompts are currently reported by this server.'
  return {
    metadata: {
      name: slug + '-ops',
      description: 'Operate ' + displayName + " through Labby's MCP gateway using its currently exposed capabilities.",
      tags: ['mcp', 'gateway', slug],
      license: '',
      compatibility: 'Labby gateway',
      allowedTools: exposedTools.map((tool) => tool.name).join(' '),
    },
    content: [
      '## When to use', '',
      'Use this skill when a task needs ' + displayName + " capabilities through Labby's MCP gateway.", '',
      '## Available tools', '', toolLines, '',
      '## Available resources', '', resourceLines, '',
      '## Available prompts', '', promptLines, '',
      '## Workflow', '',
      "1. Confirm the requested operation is within the server's exposed capabilities.",
      '2. Prefer the narrowest matching tool, resource, or prompt for the task.',
      '3. Read before writing when a read-only capability can establish current state.',
      '4. Report the operation result and any server error without inventing missing state.', '',
      '## Source', '',
      '- Gateway server: `' + gateway.name + '`',
      '- Transport: `' + gateway.transport + '`',
      '- Generated from the server catalog currently visible to Labby.',
    ].join('\n'),
  }
}
