import test from 'node:test'
import assert from 'node:assert/strict'

import { __setBrowserSessionStateForTests, getBrowserSessionState } from '../auth/session-store.ts'
import { GatewayApiError, gatewayApi } from './gateway-client.ts'
import { EXPOSE_NONE_PATTERN } from './tool-exposure-draft.ts'

type RecordedRequest = {
  action: string
  params: Record<string, unknown>
}

const standardGatewayView = {
  config: {
    name: 'gateway-1',
    url: 'http://gateway.example',
    command: null,
    args: [],
    bearer_token_env: null,
    proxy_resources: false,
    expose_tools: null,
  },
  runtime: {
    name: 'gateway-1',
    tool_count: 1,
    resource_count: 1,
    prompt_count: 1,
    exposed_tool_count: 1,
    exposed_resource_count: 1,
    exposed_prompt_count: 1,
    last_error: null,
  },
}

function jsonResponse(body: unknown) {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: {
      'content-type': 'application/json',
    },
  })
}

async function withGatewayFetch(
  handlers: Record<string, (params: Record<string, unknown>) => Promise<unknown> | unknown>,
  run: (requests: RecordedRequest[]) => Promise<void>,
) {
  const originalFetch = globalThis.fetch
  const requests: RecordedRequest[] = []
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })

  globalThis.fetch = (async (_input, init) => {
    const body = JSON.parse(String(init?.body ?? '{}')) as RecordedRequest
    requests.push(body)

    const handler = handlers[body.action]
    if (!handler) {
      throw new Error(`unexpected action: ${body.action}`)
    }

    const result = await handler(body.params)
    return result instanceof Response ? result : jsonResponse(result)
  }) as typeof fetch

  try {
    await run(requests)
  } finally {
    globalThis.fetch = originalFetch
  }
}

test('gatewayApi.create sends confirm=true with destructive gateway adds', async () => {
  await withGatewayFetch(
    {
      'gateway.add': () => standardGatewayView,
      'gateway.discovered_tools': () => ['tool.alpha'],
      'gateway.discovered_resources': () => ['lab://resource.alpha'],
      'gateway.discovered_prompts': () => ['prompt.alpha'],
    },
    async (requests) => {
      await gatewayApi.create({
        name: 'gateway-1',
        transport: 'http',
        config: {
          url: 'http://gateway.example',
        },
      } as never)

      assert.equal(
        requests.find((request) => request.action === 'gateway.add')?.params.confirm,
        true,
      )
      assert.deepEqual(requests.map((request) => request.action), [
        'gateway.add',
        'gateway.discovered_tools',
        'gateway.discovered_resources',
        'gateway.discovered_prompts',
      ])
    },
  )
})

test('gatewayApi discovery and import actions use gateway dispatch payloads', async () => {
  await withGatewayFetch(
    {
      'gateway.discover': () => [
        {
          name: 'local-files',
          source_client: 'claude-code',
          source_path: '/home/user/.claude/settings.json',
          transport: 'stdio',
          command_preview: 'npx',
          env_key_count: 1,
          already_configured: false,
        },
      ],
      'gateway.import': () => ({
        imported: [{ config: { name: 'local-files', enabled: false } }],
      }),
    },
    async (requests) => {
      const discovered = await gatewayApi.discoverExternalConfigs()
      const imported = await gatewayApi.importExternalConfigs(['local-files'])

      assert.equal(discovered[0]?.name, 'local-files')
      assert.equal(imported.imported[0]?.config.enabled, false)
      assert.deepEqual(
        requests.map((request) => ({ action: request.action, params: request.params })),
        [
          { action: 'gateway.discover', params: {} },
          { action: 'gateway.import', params: { names: ['local-files'], confirm: true } },
        ],
      )
    },
  )
})

test('gatewayApi.refreshStatus requests an explicit catalog refresh', async () => {
  await withGatewayFetch(
    { 'gateway.status': () => [] },
    async (requests) => {
      await gatewayApi.refreshStatus()
      assert.deepEqual(requests, [{ action: 'gateway.status', params: {} }])
    },
  )
})

test('gatewayApi.create adds a stdio gateway without any ack flag', async () => {
  await withGatewayFetch(
    {
      'gateway.add': () => ({
        ...standardGatewayView,
        config: {
          name: 'fixture-stdio',
          command: 'example-mcp-server',
          args: ['--stdio'],
          proxy_resources: false,
        },
      }),
      'gateway.discovered_tools': () => ['tool.alpha'],
      'gateway.discovered_resources': () => ['lab://resource.alpha'],
      'gateway.discovered_prompts': () => ['prompt.alpha'],
    },
    async (requests) => {
      await gatewayApi.create({
        name: 'fixture-stdio',
        transport: 'stdio',
        config: {
          command: 'example-mcp-server',
          args: ['--stdio'],
        },
      } as never)

      assert.equal(
        'allow_stdio' in
          (requests.find((request) => request.action === 'gateway.add')?.params ?? {}),
        false,
      )
    },
  )
})

test('gatewayApi.create sends pasted bearer tokens as a separate payload field', async () => {
  await withGatewayFetch(
    {
      'gateway.add': () => standardGatewayView,
      'gateway.discovered_tools': () => ['tool.alpha'],
      'gateway.discovered_resources': () => ['lab://resource.alpha'],
      'gateway.discovered_prompts': () => ['prompt.alpha'],
    },
    async (requests) => {
      await gatewayApi.create({
        name: 'github',
        transport: 'http',
        config: {
          url: 'https://api.githubcopilot.com/mcp/',
          bearer_token_value: 'ghp_secret',
        },
      } as never)

      assert.deepEqual(
        requests.find((request) => request.action === 'gateway.add')?.params.spec,
        {
          name: 'github',
          url: 'https://api.githubcopilot.com/mcp/',
          command: null,
          args: [],
          bearer_token_env: 'LAB_GW_GITHUB_AUTH_HEADER',
          proxy_resources: true,
          proxy_prompts: true,
          proxy_mcp_ui: true,
          expose_tools: null,
          expose_resources: null,
          expose_prompts: null,
        },
      )
      assert.equal(
        requests.find((request) => request.action === 'gateway.add')?.params.bearer_token_value,
        'ghp_secret',
      )
    },
  )
})

test('gatewayApi.update sends confirm=true with destructive gateway updates', async () => {
  await withGatewayFetch(
    {
      'gateway.update': () => standardGatewayView,
      'gateway.discovered_tools': () => ['tool.alpha'],
      'gateway.discovered_resources': () => ['lab://resource.alpha'],
      'gateway.discovered_prompts': () => ['prompt.alpha'],
    },
    async (requests) => {
      await gatewayApi.update(
        'gateway-1',
        {
          name: 'gateway-1',
          transport: 'http',
          config: {
            url: 'http://gateway-updated.example',
          },
        } as never,
      )

      assert.equal(
        requests.find((request) => request.action === 'gateway.update')?.params.confirm,
        true,
      )
      assert.deepEqual(requests.map((request) => request.action), [
        'gateway.update',
        'gateway.discovered_tools',
        'gateway.discovered_resources',
        'gateway.discovered_prompts',
      ])
    },
  )
})

test('gatewayApi.update updates a stdio gateway without any ack flag', async () => {
  await withGatewayFetch(
    {
      'gateway.update': () => ({
        ...standardGatewayView,
        config: {
          name: 'fixture-stdio',
          command: 'example-mcp-server',
          args: ['--stdio'],
          proxy_resources: false,
        },
      }),
      'gateway.discovered_tools': () => ['tool.alpha'],
      'gateway.discovered_resources': () => ['lab://resource.alpha'],
      'gateway.discovered_prompts': () => ['prompt.alpha'],
    },
    async (requests) => {
      await gatewayApi.update(
        'fixture-stdio',
        {
          name: 'fixture-stdio',
          transport: 'stdio',
          config: {
            command: 'example-mcp-server',
            args: ['--stdio'],
          },
        } as never,
      )

      assert.equal(
        'allow_stdio' in
          (requests.find((request) => request.action === 'gateway.update')?.params ?? {}),
        false,
      )
    },
  )
})

test('gatewayApi.update sends pasted bearer tokens as a separate payload field', async () => {
  await withGatewayFetch(
    {
      'gateway.update': () => standardGatewayView,
      'gateway.discovered_tools': () => ['tool.alpha'],
      'gateway.discovered_resources': () => ['lab://resource.alpha'],
      'gateway.discovered_prompts': () => ['prompt.alpha'],
    },
    async (requests) => {
      await gatewayApi.update(
        'github',
        {
          name: 'github',
          transport: 'http',
          config: {
            url: 'https://api.githubcopilot.com/mcp/',
            bearer_token_value: 'ghp_secret',
          },
        } as never,
      )

      assert.equal(
        requests.find((request) => request.action === 'gateway.update')?.params.bearer_token_value,
        'ghp_secret',
      )
    },
  )
})

test('gatewayApi.remove sends confirm=true with destructive gateway removals', async () => {
  await withGatewayFetch(
    {
      'gateway.remove': () => ({ ok: true }),
    },
    async (requests) => {
      await gatewayApi.remove('gateway-1')

      assert.equal(
        requests.find((request) => request.action === 'gateway.remove')?.params.confirm,
        true,
      )
    },
  )
})

test('gatewayApi.removeVirtualServer sends confirm=true with virtual-server removals', async () => {
  await withGatewayFetch(
    {
      'gateway.virtual_server.remove': () => ({ id: 'stale-registry' }),
    },
    async (requests) => {
      await gatewayApi.removeVirtualServer('stale-registry')

      const request = requests.find((request) => request.action === 'gateway.virtual_server.remove')
      assert.equal(request?.params.id, 'stale-registry')
      assert.equal(request?.params.confirm, true)
    },
  )
})

test('gatewayApi.reload sends confirm=true with destructive gateway reloads', async () => {
  await withGatewayFetch(
    {
      'gateway.get': () => standardGatewayView,
      'gateway.reload': () => ({ ok: true }),
    },
    async (requests) => {
      await gatewayApi.reload('gateway-1')

      assert.equal(
        requests.find((request) => request.action === 'gateway.reload')?.params.confirm,
        true,
      )
    },
  )
})

test('gatewayApi.getCodeModeConfig reads gateway-wide code mode settings', async () => {
  await withGatewayFetch(
    {
      'gateway.code_mode.get': () => ({
        enabled: true,
        timeout_ms: 2500,
        max_tool_calls: 3,
        max_response_bytes: 12000,
        max_response_tokens: 3000,
      }),
    },
    async (requests) => {
      const config = await gatewayApi.getCodeModeConfig()

      assert.deepEqual(config, {
        enabled: true,
        timeout_ms: 2500,
        max_tool_calls: 3,
        max_response_bytes: 12000,
        max_response_tokens: 3000,
      })
      assert.equal(requests[0]?.action, 'gateway.code_mode.get')
      assert.deepEqual(requests[0]?.params, {})
    },
  )
})

test('gatewayApi.setCodeModeConfig sends confirm=true for gateway-wide updates', async () => {
  await withGatewayFetch(
    {
      'gateway.code_mode.set': (params) => params,
    },
    async (requests) => {
      const config = await gatewayApi.setCodeModeConfig({
        enabled: true,
        timeout_ms: 2500,
        max_tool_calls: 3,
      })

      assert.equal(config.enabled, true)
      assert.equal(config.timeout_ms, 2500)
      assert.equal(config.max_tool_calls, 3)
      assert.equal(requests[0]?.action, 'gateway.code_mode.set')
      assert.equal(requests[0]?.params.confirm, true)
    },
  )
})

test('gatewayApi protected route actions use gateway service action payloads', async () => {
  const route = {
    name: 'tools',
    enabled: true,
    public_host: 'mcp.example.com',
    public_path: '/tools',
    backend_url: 'http://localhost:3100/mcp',
    scopes: ['mcp:read'],
    health_path: '/health',
  }

  await withGatewayFetch(
    {
      'gateway.protected_route.list_state': () => [route],
      'gateway.protected_route.get': () => route,
      'gateway.protected_route.test': () => ({
        ok: true,
        route,
        resource: 'https://mcp.example.com/tools',
        metadata_url: 'https://mcp.example.com/.well-known/oauth-protected-resource/tools',
      }),
      'gateway.protected_route.add': () => route,
      'gateway.protected_route.update': () => route,
      'gateway.protected_route.remove': () => route,
      'gateway.protected_route.stage_add': () => ({ route, restart_required: true, pending_operation: 'add', restart_note: 'restart' }),
      'gateway.protected_route.stage_update': () => ({ route, restart_required: true, pending_operation: 'update', restart_note: 'restart' }),
      'gateway.protected_route.stage_remove': () => ({ route, restart_required: true, pending_operation: 'remove', restart_note: 'restart' }),
    },
    async (requests) => {
      assert.deepEqual(await gatewayApi.listProtectedRoutes(), [route])
      assert.deepEqual(await gatewayApi.getProtectedRoute('tools'), route)
      assert.equal((await gatewayApi.testProtectedRoute(route)).ok, true)
      await gatewayApi.addProtectedRoute(route)
      await gatewayApi.updateProtectedRoute('tools', route)
      await gatewayApi.removeProtectedRoute('tools')
      assert.equal((await gatewayApi.stageProtectedRouteAdd(route)).restart_required, true)
      assert.equal((await gatewayApi.stageProtectedRouteUpdate('tools', route)).pending_operation, 'update')
      assert.equal((await gatewayApi.stageProtectedRouteRemove('tools')).pending_operation, 'remove')

      assert.deepEqual(requests.map((request) => request.action), [
        'gateway.protected_route.list_state',
        'gateway.protected_route.get',
        'gateway.protected_route.test',
        'gateway.protected_route.add',
        'gateway.protected_route.update',
        'gateway.protected_route.remove',
        'gateway.protected_route.stage_add',
        'gateway.protected_route.stage_update',
        'gateway.protected_route.stage_remove',
      ])
      assert.deepEqual(requests[1]?.params, { name: 'tools' })
      assert.deepEqual(requests[2]?.params, { route })
      assert.equal(requests[3]?.params.confirm, true)
      assert.equal(requests[4]?.params.confirm, true)
      assert.equal(requests[5]?.params.confirm, true)
      assert.equal(requests[6]?.params.confirm, true)
      assert.equal(requests[7]?.params.confirm, true)
      assert.equal(requests[8]?.params.confirm, true)
    },
  )
})

test('gatewayApi restores Loadout selection arrays the gateway omits when empty', async () => {
  // `GatewayLoadoutConfig` uses `skip_serializing_if = "Vec::is_empty"`, so a
  // Loadout that selects only upstreams omits `services` on the wire (and vice
  // versa). The Loadouts page reads `.length` off both, so an un-normalized
  // payload threw `Cannot read properties of undefined (reading 'length')` and
  // took down the whole renderer.
  const upstreamsOnly = {
    name: 'sd',
    upstreams: ['chrome-devtools'],
    expose_tools: true,
    expose_resources: true,
    expose_prompts: true,
    expose_skills: true,
    expose_code_mode: true,
  }
  const servicesOnly = {
    name: 'services-only',
    services: ['gateway'],
    expose_tools: true,
    expose_resources: true,
    expose_prompts: true,
    expose_skills: true,
    expose_code_mode: true,
  }

  await withGatewayFetch(
    {
      'gateway.loadout.list_state': () => [upstreamsOnly, servicesOnly],
      'gateway.loadout.get': () => upstreamsOnly,
      'gateway.loadout.add': () => servicesOnly,
    },
    async () => {
      const listed = await gatewayApi.listLoadouts()
      assert.deepEqual(listed[0]?.services, [])
      assert.deepEqual(listed[0]?.upstreams, ['chrome-devtools'])
      assert.deepEqual(listed[1]?.upstreams, [])
      assert.deepEqual(listed[1]?.services, ['gateway'])

      assert.deepEqual((await gatewayApi.getLoadout('sd')).services, [])
      assert.deepEqual((await gatewayApi.addLoadout({ ...servicesOnly, upstreams: [] })).upstreams, [])
    },
  )

  await withGatewayFetch(
    {
      'gateway.loadout.stage_update': () => ({
        loadout: upstreamsOnly,
        restart_required: true,
        pending_operation: 'update',
        restart_note: 'restart',
      }),
    },
    async () => {
      const staged = await gatewayApi.stageLoadoutUpdate('sd', {
        ...upstreamsOnly,
        services: [],
      })
      assert.deepEqual(staged.loadout.services, [])
      assert.deepEqual(staged.loadout.upstreams, ['chrome-devtools'])
    },
  )
})

test('gatewayApi Loadout actions use shared gateway dispatch payloads', async () => {
  const loadout = {
    name: 'operations',
    description: 'Operations agents',
    upstreams: ['github'],
    services: ['device'],
    expose_tools: false,
    expose_resources: true,
    expose_prompts: true,
    expose_skills: true,
    expose_code_mode: true,
  }

  await withGatewayFetch(
    {
      'gateway.loadout.list_state': () => [loadout],
      'gateway.loadout.get': () => loadout,
      'gateway.loadout.add': () => loadout,
      'gateway.loadout.update': () => loadout,
      'gateway.loadout.patch': () => ({ ...loadout, expose_tools: true }),
      'gateway.loadout.remove': () => loadout,
      'gateway.loadout.stage_update': () => ({ loadout, restart_required: true, pending_operation: 'update', restart_note: 'restart' }),
      'gateway.loadout.stage_patch': () => ({ loadout, restart_required: true, pending_operation: 'update', restart_note: 'restart' }),
      'gateway.loadout.stage_remove': () => ({ loadout, restart_required: true, pending_operation: 'remove', restart_note: 'restart' }),
    },
    async (requests) => {
      assert.deepEqual(await gatewayApi.listLoadouts(), [loadout])
      assert.deepEqual(await gatewayApi.getLoadout('operations'), loadout)
      await gatewayApi.addLoadout(loadout)
      await gatewayApi.updateLoadout('operations', loadout)
      const patched = await gatewayApi.patchLoadout('operations', { expose_tools: true })
      assert.equal(patched.expose_tools, true)
      await gatewayApi.removeLoadout('operations')
      assert.equal((await gatewayApi.stageLoadoutUpdate('operations', loadout)).restart_required, true)
      assert.equal((await gatewayApi.stageLoadoutPatch('operations', { expose_tools: true })).pending_operation, 'update')
      assert.equal((await gatewayApi.stageLoadoutRemove('operations')).pending_operation, 'remove')

      assert.deepEqual(requests.map((request) => request.action), [
        'gateway.loadout.list_state',
        'gateway.loadout.get',
        'gateway.loadout.add',
        'gateway.loadout.update',
        'gateway.loadout.patch',
        'gateway.loadout.remove',
        'gateway.loadout.stage_update',
        'gateway.loadout.stage_patch',
        'gateway.loadout.stage_remove',
      ])
      assert.deepEqual(requests[1]?.params, { name: 'operations' })
      assert.deepEqual(requests[4]?.params.patch, { expose_tools: true })
      assert.equal(requests[2]?.params.confirm, true)
      assert.equal(requests[3]?.params.confirm, true)
      assert.equal(requests[4]?.params.confirm, true)
      assert.equal(requests[5]?.params.confirm, true)
      assert.equal(requests[6]?.params.confirm, true)
      assert.equal(requests[7]?.params.confirm, true)
      assert.equal(requests[8]?.params.confirm, true)
    },
  )
})

test('gatewayApi.setExposurePolicy sends confirm=true when updating a gateway config', async () => {
  await withGatewayFetch(
    {
      'gateway.server.get': () => ({
        id: 'gateway-1',
        name: 'gateway-1',
        source: 'custom_gateway',
      }),
      'gateway.update': () => ({ ok: true }),
    },
    async (requests) => {
      await gatewayApi.setExposurePolicy('gateway-1', {
        mode: 'allowlist',
        patterns: ['tool.alpha'],
      })

      assert.equal(
        requests.find((request) => request.action === 'gateway.update')?.params.confirm,
        true,
      )
    },
  )
})

test('gatewayApi.getExposurePolicy preserves expose-none sentinel as empty allowlist', async () => {
  await withGatewayFetch(
    {
      'gateway.server.get': () => ({
        id: 'github-chat',
        name: 'github-chat',
        source: 'in_process',
      }),
      'gateway.virtual_server.get_mcp_policy': () => ({
        allowed_actions: [EXPOSE_NONE_PATTERN],
      }),
    },
    async () => {
      const policy = await gatewayApi.getExposurePolicy('github-chat')

      assert.deepEqual(policy, {
        mode: 'allowlist',
        patterns: [],
      })
    },
  )
})

test('gatewayApi.list does not refresh browser session for non-csrf validation errors', async () => {
  __setBrowserSessionStateForTests({
    status: 'authenticated',
    user: { sub: 'browser-user', email: 'browser@example.com' },
    expiresAt: 123,
    csrfToken: 'csrf-old',
  })

  const urls: string[] = []
  globalThis.fetch = (async (input) => {
    const url = String(input)
    urls.push(url)

    if (url === '/v1/gateway') {
      return new Response(
        JSON.stringify({
          kind: 'validation_failed',
          message: 'missing required parameter `name`',
        }),
        {
          status: 422,
          headers: {
            'content-type': 'application/json',
            'x-request-id': 'req-gateway-validation-1',
          },
        },
      )
    }

    throw new Error(`unexpected fetch: ${url}`)
  }) as typeof fetch

  await assert.rejects(
    gatewayApi.list(),
    (error: unknown) => {
      assert.ok(error instanceof GatewayApiError)
      assert.equal(error.code, 'validation_failed')
      return true
    },
  )

  assert.deepEqual(getBrowserSessionState(), {
    status: 'authenticated',
    user: { sub: 'browser-user', email: 'browser@example.com' },
    expiresAt: 123,
    csrfToken: 'csrf-old',
  })
  assert.deepEqual(urls, ['/v1/gateway'])
})

test('gatewayApi.list keeps loading when a stale in-process service is present', async () => {
  const originalWarn = console.warn
  console.warn = () => {}
  try {
    await withGatewayFetch(
      {
        'gateway.list': () => ([
          {
            id: 'missing-service',
            name: 'missing-service',
            source: 'in_process',
            configured: true,
            enabled: false,
            connected: false,
            discovered_tool_count: 0,
            exposed_tool_count: 0,
            discovered_resource_count: 0,
            exposed_resource_count: 0,
            discovered_prompt_count: 0,
            exposed_prompt_count: 0,
            surfaces: {
              cli: { enabled: false, connected: false },
              api: { enabled: false, connected: false },
              mcp: { enabled: false, connected: false },
              webui: { enabled: false, connected: false },
            },
            warnings: [],
            config_summary: {
              transport: 'in_process',
              target: 'missing-service',
            },
          },
        ]),
      },
      async () => {
        const gateways = await gatewayApi.list()

        assert.equal(gateways.length, 1)
        assert.equal(gateways[0]?.id, 'missing-service')
        assert.equal(gateways[0]?.discovery.tools.length, 0)
        assert.equal(gateways[0]?.status.discovered_tool_count, 0)
      },
    )
  } finally {
    console.warn = originalWarn
  }
})

test('gatewayApi.list logs degraded gateway row warning counts once', async () => {
  const originalWarn = console.warn
  const warnings: unknown[][] = []
  console.warn = (...args: unknown[]) => {
    warnings.push(args)
  }

  try {
    await withGatewayFetch(
      {
        'gateway.list': () => ([
          {
            id: 'missing-service',
            name: 'missing-service',
            source: 'in_process',
            configured: true,
            enabled: false,
            connected: false,
            discovered_tool_count: 0,
            exposed_tool_count: 0,
            discovered_resource_count: 0,
            exposed_resource_count: 0,
            discovered_prompt_count: 0,
            exposed_prompt_count: 0,
            surfaces: {
              cli: { enabled: false, connected: false },
              api: { enabled: false, connected: false },
              mcp: { enabled: false, connected: false },
              webui: { enabled: false, connected: false },
            },
            warnings: [
              {
                code: 'unknown_service',
                message: 'service `missing-service` is not registered in this lab binary',
              },
            ],
            config_summary: {
              transport: 'in_process',
              target: 'missing-service',
            },
          },
        ]),
      },
      async () => {
        await gatewayApi.list()

        assert.equal(warnings.length, 1)
        assert.equal(warnings[0][0], '[gateway] degraded gateway rows')
        assert.deepEqual(warnings[0][1], {
          unknown_service: 1,
        })
      },
    )
  } finally {
    console.warn = originalWarn
  }
})

test('gatewayApi.hydrateRuntime treats gateway.mcp.list as authoritative runtime state', async () => {
  await withGatewayFetch(
    {
      'gateway.mcp.list': () => [
        {
          name: 'google-drive',
          enabled: true,
          connected: false,
          discovered_tool_count: 0,
          exposed_tool_count: 0,
          discovered_resource_count: 0,
          exposed_resource_count: 0,
          discovered_prompt_count: 0,
          exposed_prompt_count: 0,
          runtime_state_path: '/home/labby/.labby/config.runtime.json',
        },
      ],
    },
    async (requests) => {
      const [gateway] = await gatewayApi.hydrateRuntime([
        {
          id: 'google-drive',
          name: 'google-drive',
          transport: 'http',
          source: 'custom_gateway',
          configured: true,
          enabled: true,
          surfaces: {
            cli: { enabled: false, connected: false },
            api: { enabled: false, connected: false },
            mcp: { enabled: true, connected: true },
            webui: { enabled: false, connected: false },
          },
          config: { url: 'https://drivemcp.googleapis.com/mcp/v1' },
          status: {
            healthy: true,
            connected: true,
            discovered_tool_count: 7,
            exposed_tool_count: 7,
            discovered_resource_count: 1,
            exposed_resource_count: 1,
            discovered_prompt_count: 0,
            exposed_prompt_count: 0,
          },
          discovery: { tools: [], resources: [], prompts: [] },
          warnings: [],
        },
      ])

      assert.equal(gateway?.status.connected, false)
      assert.equal(gateway?.surfaces?.mcp.connected, false)
      assert.equal(gateway?.status.healthy, false)
      assert.equal(gateway?.status.discovered_tool_count, 0)
      assert.equal(gateway?.status.exposed_tool_count, 0)
      assert.equal(gateway?.status.runtime_state_path, '/home/labby/.labby/config.runtime.json')
      assert.deepEqual(requests.map((request) => request.action), ['gateway.mcp.list'])
    },
  )
})

test('gatewayApi.list rethrows aborts instead of degrading rows', async () => {
  const originalWarn = console.warn
  const warnings: unknown[][] = []
  console.warn = (...args: unknown[]) => {
    warnings.push(args)
  }
  const controller = new AbortController()

  try {
    await withGatewayFetch(
      {
        'gateway.list': () => {
          controller.abort()
          throw new DOMException('Aborted', 'AbortError')
        },
      },
      async () => {
        await assert.rejects(
          () => gatewayApi.list(controller.signal),
          (error: unknown) => error instanceof DOMException && error.name === 'AbortError',
        )
        assert.equal(warnings.length, 0)
      },
    )
  } finally {
    console.warn = originalWarn
  }
})

test('gatewayApi destructive mutations send confirm=true', async () => {
  const actions: Array<{ action: string; params: Record<string, unknown> }> = []

  globalThis.fetch = (async (input, init) => {
    const url = String(input)
    if (url !== '/v1/gateway') {
      throw new Error(`unexpected fetch: ${url}`)
    }

    const payload = JSON.parse(String(init?.body ?? '{}')) as {
      action: string
      params: Record<string, unknown>
    }
    actions.push(payload)

    if (payload.action === 'gateway.get') {
      return new Response(
        JSON.stringify({
          config: { name: 'gateway_beta', proxy_resources: false },
          runtime: { tool_count: 1 },
        }),
        { status: 200, headers: { 'content-type': 'application/json' } },
      )
    }

    if (payload.action === 'gateway.mcp.list') {
      return new Response('[]', {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    }

    if (payload.action === 'gateway.add' || payload.action === 'gateway.update') {
      return new Response(
        JSON.stringify({
          config: {
            name: 'gateway_beta',
            url: 'https://lab.example.com/mcp',
            proxy_resources: false,
          },
          runtime: {
            tool_count: 1,
            resource_count: 0,
            prompt_count: 0,
            exposed_tool_count: 1,
            exposed_resource_count: 0,
            exposed_prompt_count: 0,
          },
        }),
        { status: 200, headers: { 'content-type': 'application/json' } },
      )
    }

    if (payload.action === 'gateway.remove' || payload.action === 'gateway.reload') {
      return new Response('null', {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    }

    if (
      payload.action === 'gateway.discovered_tools' ||
      payload.action === 'gateway.discovered_resources' ||
      payload.action === 'gateway.discovered_prompts'
    ) {
      return new Response('[]', {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    }

    throw new Error(`unexpected action: ${payload.action}`)
  }) as typeof fetch

  await gatewayApi.create({
    name: 'gateway_beta',
    transport: 'http',
    config: { url: 'https://lab.example.com/mcp' },
  })
  await gatewayApi.update('gateway_beta', { name: 'gateway_beta-updated' })
  await gatewayApi.remove('gateway_beta')
  await gatewayApi.reload('gateway_beta')

  const destructiveActions = actions.filter(({ action }) =>
    ['gateway.add', 'gateway.update', 'gateway.remove', 'gateway.reload'].includes(action),
  )

  assert.equal(destructiveActions.length, 4)
  for (const action of destructiveActions) {
    assert.equal(action.params.confirm, true)
  }
})

test('gatewayApi.get applies virtual-server MCP policy to in-process tool exposure', async () => {
  await withGatewayFetch(
    {
      'gateway.server.get': () => ({
        id: 'github-chat',
        name: 'github-chat',
        source: 'in_process',
        configured: true,
        enabled: true,
        connected: true,
        discovered_tool_count: 2,
        exposed_tool_count: 1,
        discovered_resource_count: 0,
        exposed_resource_count: 0,
        discovered_prompt_count: 0,
        exposed_prompt_count: 0,
        surfaces: {
          cli: { enabled: false, connected: false },
          api: { enabled: false, connected: false },
          mcp: { enabled: true, connected: true },
          webui: { enabled: false, connected: false },
        },
        warnings: [],
        config_summary: {
          transport: 'in_process',
          target: 'github-chat',
        },
      }),
      'gateway.service_config.get': () => ({
        service: 'github-chat',
        configured: true,
        fields: [],
      }),
      'gateway.service_actions': () => ([
        { name: 'index_repository', description: 'Index a GitHub repository', destructive: false },
        { name: 'query_repository', description: 'Query a GitHub repository', destructive: false },
      ]),
      'gateway.virtual_server.get_mcp_policy': () => ({
        allowed_actions: ['query_repository'],
      }),
    },
    async (requests) => {
      const gateway = await gatewayApi.get('github-chat')

      assert.deepEqual(
        gateway.discovery.tools.map((tool) => ({
          name: tool.name,
          exposed: tool.exposed,
          matched_by: tool.matched_by,
        })),
        [
          { name: 'index_repository', exposed: false, matched_by: null },
          { name: 'query_repository', exposed: true, matched_by: 'query_repository' },
        ],
      )

      assert.deepEqual(
        requests.map((request) => request.action),
        [
          'gateway.server.get',
          'gateway.service_config.get',
          'gateway.service_actions',
          'gateway.virtual_server.get_mcp_policy',
        ],
      )
    },
  )
})

test('gatewayApi.get preserves side-effect-free runtime diagnostics without testing the gateway', async () => {
  await withGatewayFetch(
    {
      'gateway.server.get': () => ({
        id: 'Asana',
        name: 'Asana',
        source: 'custom_gateway',
      }),
      'gateway.get': () => ({
        config: {
          name: 'Asana',
          url: 'https://mcp.asana.com/v2/mcp',
          enabled: true,
        },
        runtime: {
          tool_count: 0,
          resource_count: 0,
          prompt_count: 0,
          exposed_tool_count: 0,
          exposed_resource_count: 0,
          exposed_prompt_count: 0,
        },
      }),
      'gateway.discovered_tools': () => [],
      'gateway.discovered_resources': () => [],
      'gateway.discovered_prompts': () => [],
      'gateway.mcp.list': () => [{
        name: 'Asana',
        connected: false,
        pid: 4242,
        pgid: 4240,
        age_seconds: 90,
        origin: 'gateway_pool',
        runtime_state_path: '/tmp/gateway.runtime.json',
        reconciled_at: '2026-08-22T05:00:00Z',
        likely_stale_count: 1,
      }],
    },
    async (requests) => {
      const gateway = await gatewayApi.get('Asana')

      assert.equal(gateway.name, 'Asana')
      assert.equal(gateway.status.connected, false)
      assert.equal(gateway.status.pid, 4242)
      assert.equal(gateway.status.pgid, 4240)
      assert.equal(gateway.status.runtime_state_path, '/tmp/gateway.runtime.json')
      assert.equal(gateway.status.reconciled_at, '2026-08-22T05:00:00Z')
      assert.equal(gateway.status.likely_stale_count, 1)
      assert.deepEqual(
        requests.map((request) => request.action),
        [
          'gateway.server.get',
          'gateway.get',
          'gateway.mcp.list',
          'gateway.discovered_tools',
          'gateway.discovered_resources',
          'gateway.discovered_prompts',
        ],
      )
    },
  )
})
