import type { GatewayLoadout } from '../../lib/types/gateway.ts'

export type LoadoutExportTarget = 'apm' | 'claude-code' | 'codex' | 'gemini-cli'

export function portableLoadoutManifest(loadout: GatewayLoadout, target: LoadoutExportTarget = 'apm') {
  return {
    schema: 'https://apm.dev/schemas/loadout/v1',
    kind: 'loadout',
    metadata: {
      name: loadout.name,
      ...(loadout.description ? { description: loadout.description } : {}),
    },
    spec: {
      target,
      mcpServers: [...loadout.upstreams].sort((left, right) => left.localeCompare(right)),
      plugins: [...loadout.services].sort((left, right) => left.localeCompare(right)),
      artifacts: {
        tools: loadout.expose_tools,
        resources: loadout.expose_resources,
        prompts: loadout.expose_prompts,
        skills: loadout.expose_skills,
        codeMode: loadout.expose_code_mode,
      },
    },
  }
}

export function portableLoadoutSource(loadout: GatewayLoadout, target: LoadoutExportTarget = 'apm'): string {
  return `${JSON.stringify(portableLoadoutManifest(loadout, target), null, 2)}\n`
}

export function portableLoadoutFilename(loadout: GatewayLoadout, target: LoadoutExportTarget = 'apm'): string {
  const safeName = loadout.name.trim().toLocaleLowerCase().replace(/[^a-z0-9._-]+/g, '-').replace(/^-+|-+$/g, '') || 'loadout'
  return target === 'apm' ? `${safeName}.loadout.json` : `${safeName}.${target}.loadout.json`
}
