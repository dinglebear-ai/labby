# Agent setup

Ask before scanning/changing clients. Start read-only with `labby gateway discover --json`; supplement with executable/config checks and show changes first.

Prefer client CLIs/current docs. Codex: check `codex mcp --help`, register Streamable HTTP, use env-backed bearer or OAuth, verify `codex mcp list`. Claude: check `claude mcp --help`; the packaged static header is bearer-only, so OAuth must omit it. Never track tokens. Reload and prove a fresh read-only call.
