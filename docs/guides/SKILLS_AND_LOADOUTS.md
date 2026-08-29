---
title: "Agent Skills and Loadouts"
created: "2026-08-18"
updated: "2026-08-18"
---

# Agent Skills and Loadouts

This guide is the operator and contributor contract for Agent Skills over MCP and reusable Gateway Loadouts. Both features live in the shared gateway/dispatch path so CLI, POST /v1/gateway, MCP management calls, and the Gateway Admin webapp use the same actions and validation.

## Agent Skills trust model

Labby treats upstream Agent Skills as an explicit trust boundary. An upstream can advertise the MCP Skills extension io.modelcontextprotocol/skills without Labby automatically trusting its agent instructions.

The upstream fields are:

- proxy_skills = false: default; do not aggregate Skills from this upstream.
- proxy_skills = true: trust the upstream as a Skill source and allow enumeration/validation.
- expose_skills = null: expose every validated Skill from a trusted upstream.
- expose_skills = []: expose no Skills from that trusted upstream.
- expose_skills = [patterns]: expose only exact or wildcard skill-name matches.

The empty-list behavior is intentionally different from the legacy tool/resource/prompt filters. Converting an empty Skill allowlist into null would invert an operator deny-all decision into expose-all.

## Skills CLI

~~~bash
labby gateway skills list
labby gateway skills list --upstream github
labby gateway skills trust github --yes
labby gateway skills untrust github
labby gateway skills expose github --pattern 'review-*' --pattern deploy
labby gateway skills expose-all github
~~~

The generic upstream commands expose the same dispatch-backed configuration fields:

~~~bash
labby gateway add --name github --url https://mcp.example.com --proxy-skills true --expose-skill 'review-*'
labby gateway update github --proxy-skills true --expose-skill 'review-*'
labby gateway update github --clear-expose-skills
~~~

## Skills API and dispatch

All HTTP management goes through the authenticated POST /v1/gateway action envelope. MCP and CLI adapters call the same gateway dispatch implementation.

~~~json
{ "action": "gateway.skills.list", "params": {} }
~~~

~~~json
{ "action": "gateway.skills.list", "params": { "upstream": "github" } }
~~~

Trust/exposure mutations use gateway.update:

~~~json
{
  "action": "gateway.update",
  "params": {
    "name": "github",
    "patch": {
      "proxy_skills": true,
      "expose_skills": ["review-*"]
    }
  }
}
~~~

The operator listing returns a row per relevant upstream with enabled/trusted
state, extension support, validated Skills, rejected entries, excluded count,
truncation, cache age, and a per-upstream error when inspection degrades. Each
validated Skill retains the legacy `exposed` boolean and also reports its
provider-scoped identity plus the structured exposure decision:

~~~json
{
  "identity": {
    "provider": { "kind": "mcp_sep", "instance": "github" },
    "source_id": "skill://github/review/SKILL.md"
  },
  "exposed": true,
  "exposure": {
    "status": "exposed",
    "reason": "matched_pattern",
    "matched_pattern": "review-*"
  }
}
~~~

Identity is provenance, not authorization. The exposure decision explains the
existing `expose_skills` policy result; it does not grant tool access.

## Skills graceful degradation

Skills discovery is intentionally partial-result friendly.

- One unreachable upstream does not erase healthy upstream results.
- The downstream skills/list aggregate annotates incomplete results with metadata such as unreachableUpstreams, excludedSkills, and truncated.
- skills/get resolves against the same aggregate and can fall back to a manifest-bound direct upstream read for a valid Skill URI that was omitted by a partial listing.
- Skill resource reads enforce route, trust, and exposure policy again. A hidden URI cannot bypass skills/list by being read directly.
- Unknown gateway.skills.list upstream filters return not_found with guidance to list valid upstream names.
- A missing gateway runtime returns runtime_unavailable with guidance to start/reconnect labby serve.
- A build without the Skills feature returns a corrective feature/release-build message.

## Skills logging

Operator Skills inspection emits structured dispatch events including surface, service, action, upstream, trust/support state, discovery/exposure counts, rejected count, truncation, cache age, elapsed time, and error kind where relevant.

The MCP skills/list and skills/get paths emit structured start/finish/error events. Partial upstream failures are logged without turning a healthy aggregate into a total failure. Skill bodies, credentials, and bearer values are not logged.

## Loadouts

A Loadout is a named reusable gateway capability projection for protected gateway-subset routes. It is not a second per-upstream allowlist system. A Loadout can only narrow what the route sees; upstream trust and per-upstream exposure rules still apply underneath.

Each Loadout contains:

- name and optional description
- selected upstream MCP server names
- selected built-in Lab service names
- expose_tools
- expose_resources
- expose_prompts
- expose_skills
- expose_code_mode

Agent Skills require Resources because Skill files are retrieved through MCP resources. A Loadout with Skills enabled and Resources disabled is rejected before persistence/mounting.

## Loadouts CLI

~~~bash
labby gateway loadout list
labby gateway loadout get operations
labby gateway loadout add operations --upstream github --service device --code-mode
labby gateway loadout update operations --expose-tools false --expose-skills true
labby gateway loadout update operations --clear-upstreams --service device
# Mounted Loadouts are staged instead of hot-swapped:
labby gateway loadout update operations --expose-tools false --stage-for-restart
labby gateway loadout remove operations --stage-for-restart
~~~

Loadout add defaults Tools, Resources, Prompts, and Skills to enabled while Code Mode remains disabled. Initial narrowing flags are --no-tools, --no-resources, --no-prompts, and --no-skills.

Loadout update uses patch semantics. Unspecified fields remain unchanged. Explicit empty upstream/service selections clear those selections. A cleared description becomes null. If an enabled protected route is currently mounted with the Loadout, direct update/remove stays fail-closed with restart_required; use --stage-for-restart to persist the desired Loadout change without claiming the running route projection changed.

## Loadouts API and dispatch

~~~json
{ "action": "gateway.loadout.list_state", "params": {} }
~~~

~~~json
{
  "action": "gateway.loadout.add",
  "params": {
    "loadout": {
      "name": "operations",
      "description": "Operations agents",
      "upstreams": ["github"],
      "services": ["device"],
      "expose_tools": false,
      "expose_resources": true,
      "expose_prompts": true,
      "expose_skills": true,
      "expose_code_mode": true
    }
  }
}
~~~

Use gateway.loadout.patch when only selected fields should change:

~~~json
{
  "action": "gateway.loadout.patch",
  "params": {
    "name": "operations",
    "patch": {
      "expose_tools": false,
      "expose_skills": true
    }
  }
}
~~~

gateway.loadout.update remains the whole-object replacement action for callers that intentionally want replacement semantics. For mounted Loadouts, gateway.loadout.stage_update and gateway.loadout.stage_patch persist desired config only and return restart_required=true. gateway.loadout.list_state compares desired Loadouts with the running process projection and surfaces pending update/remove state.

To remove a Loadout still used by the running router in one restart cycle, stage every desired protected route away from that Loadout first, then call gateway.loadout.stage_remove. The backend rejects the Loadout removal while any desired protected route still references it, while still recognizing that the running route may retain the old projection until restart.

## Protected route integration

A gateway-subset protected route may target a named Loadout instead of embedding upstream/service selection inline.

~~~json
{
  "action": "gateway.protected_route.add",
  "params": {
    "route": {
      "name": "operations",
      "enabled": true,
      "public_host": "mcp.example.com",
      "public_path": "/operations",
      "upstream": null,
      "backend_url": "",
      "scopes": ["mcp:read"],
      "health_path": null,
      "target": {
        "kind": "gateway_subset",
        "loadout": "operations"
      }
    }
  }
}
~~~

A Loadout target is mutually exclusive with inline target upstreams, target services, and inline Code Mode exposure. One route has one authoritative projection source.

Protected gateway-subset routes are mounted when labby serve starts. The direct add/update/remove actions therefore still return restart_required rather than pretending the live router changed. First-class control-plane clients use the staged route actions for these mutations: gateway.protected_route.stage_add, stage_update, and stage_remove. Staging writes the desired durable config but deliberately leaves the running route set untouched. The response derives restart_required and pending_operation from desired-vs-runtime state, so cancelling a staged change can correctly report that no restart remains. gateway.protected_route.list_state uses the same comparison and reports pending add/update/remove state until the next restart.

The WebUI automatically stages Loadout/gateway-subset route changes and labels them Restart · add/update/remove. Once any protected-route change crosses a gateway-subset boundary, the running process freezes the complete protected-route collection at its startup revision until restart; follow-up direct route edits are staged into the same desired transaction rather than half-publishing a rename or related change. The CLI automatically stages additions and updates whose replacement target is a gateway subset; use --stage-for-restart when updating a currently-mounted subset to a direct route, removing a mounted subset, or continuing route edits while restart debt is already pending. Restart labby serve to apply the desired route set. A referenced Loadout cannot be removed until all protected routes, including disabled routes, stop referencing it.

## Loadout validation and course correction

Loadout validation fails closed with actionable errors:

- duplicate or empty names are rejected
- unknown upstreams tell the caller to add the upstream first or remove it from the Loadout
- reserved in-process upstream ids tell the caller to use the services selection instead
- unknown Lab services tell the caller to inspect gateway.supported_services
- zero enabled capability categories are rejected
- Skills without Resources are rejected with the corrective choice
- protected routes naming a missing Loadout tell the caller to create the Loadout or update the route
- removing a referenced Loadout identifies the route names that must be updated first

## MCP category gates

A Loadout is enforced at the real MCP boundaries, not only in UI/config.

- Tools disabled: direct upstream and Lab service tools are omitted/denied. Code Mode remains available when its separate gate is enabled.
- Resources disabled: resources/list and resource-template listing return empty; resources/read is denied with a Loadout-specific message. Text Code Mode may remain enabled, but Labby suppresses codemode_ui and strips resource-backed MCP App bindings from advertised tools and tool results so clients are never pointed at UI resources this route cannot read.
- Prompts disabled: prompts/list returns empty and prompts/get is denied with a Loadout-specific message.
- Skills disabled: skills/list returns an empty private/no-cache catalog, skills/get is denied, and Skill resource reads are denied.
- Code Mode disabled: existing route-scope Code Mode denial remains authoritative.

List operations degrade to empty catalogs where that is the least surprising MCP behavior; direct reads/gets/calls return useful denials so agents can course-correct instead of mistaking policy for disappearance.

## Webapp

Gateway Admin exposes Loadouts as a first-class Control Plane route at /loadouts. The page uses the same Aurora page shell, ConsoleHero, DashboardPanel, Dialog, Switch, Checkbox, Badge, Button, and confirmation patterns used elsewhere in the console.

## Execution loadouts

Execution loadouts are a separate per-turn domain. They do not reuse or alter
`GatewayLoadoutConfig`, which remains the mounted-route configuration and
restart-debt contract described above. An execution loadout stores one bounded,
deterministically normalized collection of provider-qualified references for
tools, prompts, resources, skills, agents, MCP apps, MCP servers, and plugins.
Each reference contains a stable provider identity, family, opaque member ID,
and expected member revision; display names never grant access.

Authenticated clients use the bounded Palette REST surface:

- `GET|POST /v1/palette/execution-loadouts` lists or creates drafts;
- `GET|PATCH /v1/palette/execution-loadouts/{id}` reads or CAS-revises a draft;
- `POST .../{id}/preview` resolves a side-effect-free principal/runtime-bound
  preview against one catalog generation;
- `POST .../{id}/activate` re-resolves the live catalog and authorization before
  atomically creating an immutable active revision; and
- `POST .../{id}/rollback` copies an immutable revision into a new draft revision.

Draft, desired active, and effective runtime revisions are distinct. Execution
loadouts never create route restart debt. Preview and activation report missing,
stale, unauthorized, and unsupported references explicitly. Families that do
not yet have authoritative live catalog identities remain selectable but fail
closed as `unsupported`; they are never silently discarded. Catalog search,
lazy schema/descriptor hydration, and result sizes retain the Palette endpoint's
existing server-side limits, so clients do not download or expand the catalog.

The Loadout form provides upstream/service selection and category gates. It blocks invalid Skills-without-Resources combinations before submit while backend validation remains authoritative.

Settings → Surfaces mounts the Protected MCP Routes editor. It can select a named Loadout; when selected, direct upstream/backend inputs are disabled and the route payload uses the gateway_subset Loadout target. Gateway-subset route changes are staged automatically and display Restart · add/update/remove until the process restarts. The Loadouts page similarly stages edits to mounted Loadouts and shows a restart banner/badge instead of presenting desired config as live.

The Gateway list/detail surfaces include discovered/exposed Skill counts alongside tools, resources, and prompts. The Skills catalog surfaces trust state, per-upstream degradation, exclusions, truncation, and cache age.

## Verification

Relevant focused checks:

~~~bash
cargo check -p labby-gateway --features skills --locked
cargo check -p labby --all-features --locked
cargo test -p labby-gateway --features skills --locked loadout_
cargo test -p labby-gateway --features skills --locked protected_route_rejects_unknown_loadout
cargo test -p labby-gateway --features skills --locked update_upstream_applies_and_clears_expose_skills
cargo test -p labby --lib --features skills --locked route_scope
cargo test -p labby --lib --features skills --locked gateway_cli_parser_accepts_expected_commands
pnpm --dir apps/gateway-admin exec tsc --noEmit
pnpm --dir apps/gateway-admin run test:unit
pnpm --dir apps/gateway-admin run lint
~~~
