import type {
  Gateway,
  ServiceAction,
  ServiceConfig,
  SupportedService,
} from '../types/gateway.ts'
import {
  mockGateways,
  mockServiceActions,
  mockServiceConfigs,
  mockSupportedServices,
} from './mock-data.ts'
import { applyMockGatewayOverride, getMockGatewayOverride } from './mock-gateway-overrides.ts'
import { matchPattern } from './exposure-policy-matcher.ts'
import type { UpstreamSkillsRow } from './skills-model.ts'

function cloneValue<T>(value: T): T {
  return structuredClone(value)
}

export function getMockGatewaysFallback(): Gateway[] {
  return cloneValue(mockGateways).map((gateway) =>
    applyMockGatewayOverride(gateway, getMockGatewayOverride(gateway.id)),
  )
}

export function getMockGatewayFallback(id: string): Gateway | undefined {
  const gateway = mockGateways.find((item) => item.id === id)
  return gateway ? applyMockGatewayOverride(cloneValue(gateway), getMockGatewayOverride(id)) : undefined
}

export function getMockSupportedServicesFallback(): SupportedService[] {
  return cloneValue(mockSupportedServices)
}

export function getMockServiceConfigFallback(service: string): ServiceConfig {
  return cloneValue(mockServiceConfigs[service] ?? { service, configured: false, fields: [] })
}

export function getMockServiceActionsFallback(service: string): ServiceAction[] {
  return cloneValue(mockServiceActions[service] ?? [])
}

const MOCK_SKILLS: Record<string, Array<{ name: string; uri: string; description: string; resource_count: number }>> = {
  'github-server': [
    {
      name: 'review-pr',
      uri: 'skill://github-server/review-pr/SKILL.md',
      description: 'Review a pull request with repository context and checks.',
      resource_count: 2,
    },
    {
      name: 'release-notes',
      uri: 'skill://github-server/release-notes/SKILL.md',
      description: 'Draft release notes from merged pull requests.',
      resource_count: 1,
    },
  ],
  'slack-server': [
    {
      name: 'incident-summary',
      uri: 'skill://slack-server/incident-summary/SKILL.md',
      description: 'Summarize an incident channel into timeline, impact, and follow-ups.',
      resource_count: 2,
    },
  ],
}

const MOCK_SKILL_SUPPORT: Record<string, boolean | null> = {
  'filesystem-server': false,
  'github-server': true,
  'slack-server': true,
  'database-server': null,
  'memory-server': false,
}

function mockSkillExposed(name: string, patterns: string[] | null | undefined): boolean {
  if (patterns == null) return true
  return patterns.some((pattern) => matchPattern(name, pattern))
}

export function getMockSkillsRowsFallback(upstream?: string): UpstreamSkillsRow[] {
  return getMockGatewaysFallback()
    .filter((gateway) => !upstream || gateway.name === upstream)
    .map((gateway) => {
      const supportsSkills = MOCK_SKILL_SUPPORT[gateway.name] ?? null
      const trusted = gateway.config.proxy_skills === true
      const catalog = trusted && supportsSkills === true ? (MOCK_SKILLS[gateway.name] ?? []) : []
      const skills = catalog.map((skill) => ({
        ...skill,
        exposed: mockSkillExposed(skill.name, gateway.config.expose_skills),
      }))
      const rejected = gateway.name === 'github-server' && trusted
        ? [{ uri: 'skill://github-server/unsigned-helper/SKILL.md', reason: 'digest_mismatch' }]
        : []
      return {
        upstream: gateway.name,
        enabled: gateway.enabled ?? true,
        trusted,
        supports_skills: supportsSkills,
        exposure_patterns: gateway.config.expose_skills ?? null,
        skills,
        discovered_count: skills.length,
        exposed_count: skills.filter((skill) => skill.exposed).length,
        rejected,
        excluded_count: rejected.length,
        truncated: false,
        cache_age_secs: skills.length ? 8 : 0,
        error: null,
      }
    })
}
