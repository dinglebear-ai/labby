---
name: homelab-readonly-pulse
title: "Homelab Read-Only Pulse"
created: "2026-07-30"
updated: "2026-09-16"
description: Read-only homelab pulse across time, Docker, Cortex, and Synapse
tags: [homelab, readonly, ops]
inputs:
  timezone:
    type: string
    default: America/New_York
    required: false
    description: IANA timezone for the timestamp call
  log_query:
    type: string
    default: error
    required: false
    description: Cortex log search query
  log_limit:
    type: integer
    default: 5
    required: false
    description: Maximum logs to include
  container_sample:
    type: integer
    default: 12
    required: false
    description: Maximum containers to sample
---

# Homelab Read-Only Pulse

Use this snippet for a compact, read-only infrastructure pulse using upstreams
that are currently discoverable through Labby. It intentionally avoids historical
alias namespaces such as `rustify`, `rustscale`, `rustifi`, and `unrust`.
Add optional services only after rediscovering and inspecting their live schemas.

## Current Calls

| Evidence | Tool | Parameters |
| --- | --- | --- |
| Timestamp | `time::get_current_time` | `timezone` |
| Docker hosts | `dozzle::list_hosts` | none |
| Docker containers | `dozzle::list_containers` | optional server-side `state`; this snippet samples client-side |
| Recent logs | `cortex::cortex` | `action: "search"`, `query`, `limit` |
| Synapse nodes | `synapse::scout` | `action: "nodes"` |
| Synapse host status | `synapse::flux` | `action: "host"`, `subaction: "status"` |

These six calls were rediscovered from the live catalog and smoke-tested through
Labby on 2026-09-16. They are independent, so the snippet uses
`codemode.batch` and preserves each call's own failure instead of allowing one
upstream error to discard the rest of the pulse.

## Validation Expectations

- `time::get_current_time.timezone` is a string.
- `cortex::cortex.action` is `search`; `query` is a string and `limit` is numeric.
- `synapse::scout.action` is `nodes`.
- `synapse::flux.action` is `host` with `subaction: "status"`.
- The snippet does not claim notification, Tailscale, UniFi, Unraid, or Arcane
  coverage unless those upstream tools are rediscovered for the active route.

Run with:

```bash
labby gateway code exec --json --code "$(awk '/^```js$/{flag=1;next}/^```$/{if(flag){exit}}flag' docs/snippets/homelab-readonly-pulse.md)"
```

```js
async (overrides = {}) => {
  const input = {
    timezone: overrides.timezone ?? "America/New_York",
    logQuery: overrides.log_query ?? "error",
    logLimit: overrides.log_limit ?? 5,
    containerSample: overrides.container_sample ?? 12
  };

  const timed = async (label, id, params, transform = (value) => value) => {
    const started = Date.now();
    try {
      const result = await callTool(id, params);
      return {
        label,
        id,
        ok: true,
        ms: Date.now() - started,
        result: transform(result)
      };
    } catch (error) {
      return {
        label,
        id,
        ok: false,
        ms: Date.now() - started,
        error: String(error)
      };
    }
  };

  const jobs = [
    () => timed("timestamp", "time::get_current_time", { timezone: input.timezone }),
    () => timed(
      "docker_hosts",
      "dozzle::list_hosts",
      {},
      (hosts) => (hosts || []).map((host) => ({
        name: host.name,
        available: host.available,
        type: host.type,
        dockerVersion: host.dockerVersion,
        cpu: host.nCPU,
        memTotal: host.memTotal
      }))
    ),
    () => timed(
      "docker_containers",
      "dozzle::list_containers",
      {},
      (containers) => {
        const list = containers || [];
        const byState = {};
        const byHost = {};
        for (const container of list) {
          byState[container.state] = (byState[container.state] || 0) + 1;
          byHost[container.host] = (byHost[container.host] || 0) + 1;
        }
        return {
          total: list.length,
          byState,
          byHost,
          sample: list.slice(0, input.containerSample).map((container) => ({
            name: container.name,
            image: container.image,
            state: container.state,
            host: container.host
          }))
        };
      }
    ),
    () => timed(
      "recent_logs",
      "cortex::cortex",
      { action: "search", query: input.logQuery, limit: input.logLimit },
      (result) => ({
        count: result.count,
        logs: (result.logs || []).slice(0, input.logLimit).map((log) => ({
          timestamp: log.timestamp,
          hostname: log.hostname,
          app_name: log.app_name,
          severity: log.severity,
          message: log.message
        }))
      })
    ),
    () => timed("synapse_nodes", "synapse::scout", { action: "nodes" }),
    () => timed(
      "synapse_host_status",
      "synapse::flux",
      { action: "host", subaction: "status" }
    )
  ];

  const batch = await codemode.batch(jobs);
  const calls = batch.ok
    .sort((a, b) => a.i - b.i)
    .map((entry) => entry.value);
  calls.push(...batch.failed.map((entry) => ({
    label: `batch_job_${entry.i}`,
    id: "codemode.batch",
    ok: false,
    error: String(entry.error)
  })));

  const byLabel = Object.fromEntries(calls.map((call) => [call.label, call]));
  const containers = byLabel.docker_containers?.result;
  const degraded = calls.filter((call) => !call.ok);

  return {
    snippet: "homelab_readonly_pulse",
    input,
    ok: degraded.length === 0,
    status: degraded.length === 0 ? "ok" : "degraded",
    summary: {
      docker_hosts: byLabel.docker_hosts?.result?.length,
      docker_containers: containers?.total,
      docker_container_states: containers?.byState,
      recent_log_count: byLabel.recent_logs?.result?.count
    },
    degraded: degraded.map((call) => ({ label: call.label, id: call.id, error: call.error })),
    calls
  };
}
```
