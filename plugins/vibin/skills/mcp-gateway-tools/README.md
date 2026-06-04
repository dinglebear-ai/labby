# mcp-gateway-tools

How to invoke upstream MCP tools through the Lab gateway when the exposed MCP tools are `search` and `execute`.

## What it does

Encodes the gateway's name-resolution + invocation contract for agents:
- Discover exact `<upstream>::<tool>` ids with `search({ code })`, then call them from `execute({ code })`.
- Keep the current JavaScript call shape straight; legacy `tool_execute({ name, arguments })` is not the canonical surface.
- Recognize and respond to the canonical error envelope (`unknown_action`, `index_warming`, etc.) without guessing-and-retrying.
- Tune search queries when the wrong tool is surfacing first.

## Invoke

Automatically — if `list_tools` shows the synthetic `*__search` + `*__execute` pair and the descriptions mention Code Mode. Also triggers on phrases like "find a tool for...", "call X through the gateway".

## Files

- `SKILL.md` — error table, anti-patterns, quick reference
