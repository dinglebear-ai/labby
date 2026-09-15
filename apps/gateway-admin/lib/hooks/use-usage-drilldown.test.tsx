import assert from 'node:assert/strict'
import test from 'node:test'
import React, { act } from 'react'
import { SWRConfig } from 'swr'

import { __setBrowserSessionStateForTests } from '@/lib/auth/session-store.ts'
import { installTestDom, renderClient } from '@/lib/testing/dom-test-utils.tsx'
import { useToolCalls } from './use-usage-drilldown.ts'

installTestDom()

function metrics(version: string) {
  return {
    attribution_filters: { client_name: 'Codex CLI', client_version: version },
    window_total_calls: 2,
    total_calls: 1,
    error_calls: 0,
    avg_elapsed_ms: 10,
    p50_elapsed_ms: 10,
    p95_elapsed_ms: 10,
    p99_elapsed_ms: 10,
    distinct_tools: 1,
    distinct_actors: 1,
    peak_per_min: 1,
    top_tools: [{ upstream: 'client', tool: version, calls: 1, failed: 0 }],
    least_tools: [{ upstream: 'client', tool: version, calls: 1, failed: 0 }],
    top_actors: [{ actor: 'unattributed', calls: 1 }],
    slowest_tools: [{ upstream: 'client', tool: version, avg_elapsed_ms: 10 }],
    errors: [],
    upstreams: [{ upstream: 'client', calls: 1, failed: 0 }],
    hourly: Array.from({ length: 24 }, (_, hour) => ({ hour, calls: 0 })),
    timeseries: [],
    facets: {
      tools: [{ upstream: 'client', tool: version }],
      actors: ['unattributed'],
      upstreams: ['client'],
      outcomes: ['ok'],
    },
  }
}

function calls(version: string) {
  return {
    attribution_filters: { client_name: 'Codex CLI', client_version: version },
    calls: [{
      ts_unix: 1_800_000_000,
      upstream: 'client',
      tool: version,
      actor: 'unattributed',
      outcome: 'ok',
      elapsed_ms: 10,
      attribution: {
        inbound_actor: 'unattributed',
        actor_kind: 'client',
        client_name: 'Codex CLI',
        client_version: version,
      },
    }],
    total_matching: 1,
  }
}

function CallsProbe({ version }: { version: string }) {
  const { data, isLoading } = useToolCalls({
    window: '24h',
    agent: 'unattributed',
    client_name: 'Codex CLI',
    client_version: version,
  })
  return <output>{data?.calls[0]?.tool ?? (isLoading ? 'loading' : 'empty')}</output>
}

function probe(version: string) {
  return (
    <SWRConfig value={{ provider: () => new Map(), dedupingInterval: 0 }}>
      <CallsProbe version={version} />
    </SWRConfig>
  )
}

async function waitFor(assertion: () => void, timeoutMs = 2_000) {
  const deadline = Date.now() + timeoutMs
  let lastError: unknown
  while (Date.now() < deadline) {
    try {
      assertion()
      return
    } catch (error) {
      lastError = error
    }
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 5)) })
  }
  throw lastError
}

test('an exact client-version switch does not present previous rows while the new request is pending', async () => {
  __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  const originalFetch = globalThis.fetch
  const pending: Array<{ action: string; resolve: (response: Response) => void }> = []
  globalThis.fetch = async (_input, init) => {
    const request = JSON.parse(String(init?.body)) as {
      action: string
      params: { client_version?: string }
    }
    const version = request.params.client_version ?? 'missing'
    if (version === '1.0') {
      return Response.json(request.action === 'gateway.usage.metrics' ? metrics(version) : calls(version))
    }
    return new Promise<Response>((resolve) => pending.push({ action: request.action, resolve }))
  }

  document.body.replaceChildren()
  const view = await renderClient(probe('1.0'))
  try {
    await waitFor(() => assert.equal(view.container.textContent, 'client::1.0'))
    await view.rerender(probe('2.0'))
    await waitFor(() => assert.equal(pending.length, 2))
    assert.equal(view.container.textContent, 'loading')
    assert.doesNotMatch(view.container.textContent ?? '', /client::1\.0/)

    for (const request of pending) {
      request.resolve(Response.json(
        request.action === 'gateway.usage.metrics' ? metrics('2.0') : calls('2.0'),
      ))
    }
    await waitFor(() => assert.equal(view.container.textContent, 'client::2.0'))
  } finally {
    await view.unmount()
    globalThis.fetch = originalFetch
    __setBrowserSessionStateForTests({ status: 'unauthenticated' })
  }
})
