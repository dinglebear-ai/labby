# Token-Efficient CLI Formatting

## Overview

`lab` should present dense, readable CLI output without hiding data. The goal is not to summarize away details, but to structure them so a human can scan the result quickly while the underlying command still preserves the full machine-readable payload.

This document captures the formatting patterns that work well for the CLI and removes the old MCP-server-specific assumptions. The useful part of the old design is the output style itself:

- keep full data available in structured output
- render a compact human summary first
- group related values instead of repeating rows
- use stable symbols and aligned columns
- keep logs compact by default and defer full detail to JSON or raw output

## Architecture

The CLI should treat formatting as a boundary concern:

1. command handlers gather data
2. a central formatter turns that data into a human-readable summary
3. the same command can still emit full structured output when requested

That means business logic stays in service/client code, while the CLI layer owns presentation. The formatter should be responsible for:

- table layout
- grouping and truncation rules
- status symbols
- summary lines and counts
- optional log previews

The most important rule is simple: do not reduce the data set just to make the output smaller. Reduce repetition, not information.

## Formatting Patterns

### Port Mappings

Best fit for port-heavy output: group by container, then list mappings on one line.

Example:

```text
Port Usage on node-b
Found 82 exposed ports across 41 containers

Protocols: TCP: 78, UDP: 4
Port ranges: System: 14, User: 68, Dynamic: 0

PORT MAPPINGS:
  swag [swag]: 2002->22/tcp, 443->443/tcp, 80->80/tcp
  adguard [adguard]: 3000->3000/tcp, 53->53/tcp, 53->53/udp, 3010->80/tcp
```

Style rules:

- group ports by container
- keep host -> container mapping in a compact form
- use a warning marker for conflicts or collisions
- keep protocol lowercase in the rendered text

### Host Listings

Use a compact table with one row per host and a stable column order.

Example:

```text
Docker Hosts (7 configured)
Host         Address              ZFS Dataset
------------ -------------------- -----------
controller       controller:29229         cache/appdata
backup-node        backup-node:22             backup/appdata
node-b      node-b:22           rpool/appdata
vivobook-wsl vivobook-wsl:22      -
```

Style rules:

- show the count in the title
- keep columns short and fixed-width
- use `-` for missing values
- avoid repeating obvious labels in each row

### Container Listings

Use a single-line summary per container with status, port summary, and project name.

Example:

```text
Docker Containers on node-b
Showing 20 of 41 containers

  Container                 Ports                Project
  ------------------------- -------------------- ---------------
  running swag-mcp          8012                 swag-mcp
  running syslog-ng         514,601+6            syslog-mcp
  stopped elasticsearch     -                    syslog-mcp
```

Style rules:

- keep state visible at a glance
- compress port lists after the first few entries
- show overflow as `+N`
- truncate long names consistently

### Stack Listings

Use a summary table with status breakdown up top and per-stack service counts below.

Example:

```text
Docker Compose Stacks on node-b (28 total)
Status breakdown: running: 27, partial: 1

  Stack                     Status     Services
  ------------------------- ---------- ---------------
  running swag-mcp          running    [1] swag-mcp
  partial syslog-mcp        partial    [3] syslog-ng, elasticsearch...
  running authelia          running    [3] authelia, authelia-redis...
```

Style rules:

- surface the status distribution first
- show service count in brackets
- list only the first two services, then append `...`
- preserve a separate status symbol or word, not both if it becomes noisy

### Logs

Logs should be compact by default. The CLI can show a small preview with counts and truncation hints, while the full log payload stays available in structured output.

Example:

```text
Container Logs for swag on node-b
Lines returned: 100 (requested: 100)
truncated: false | follow: false

Preview (first 5):
  [..] line 1
  [..] line 2
  [..] line 3
  [..] line 4
  [..] line 5
```

Style rules:

- show how many lines were returned
- show whether the output was truncated
- preview only a small, predictable slice
- avoid flooding the terminal with raw logs unless explicitly requested

### CRUD Summaries

For add/edit/remove/test operations, keep confirmation output short and structured.

Example:

```text
Host added: prod (prod.example.com)
SSH: docker@prod.example.com:22 | tested: yes

Host updated: prod
Fields: ssh_user, ssh_port, zfs_capable

Host removed: prod (prod.example.com)

SSH OK: prod prod.example.com:22
Docker: 24.0.6
```

Style rules:

- one or two lines is usually enough
- include the key identifiers
- use yes/no or pass/fail consistently
- keep the full response available elsewhere if the action returns richer data

### Discovery Output

Discovery commands should summarize scope first, then list the most useful locations or candidates.

Example:

```text
Compose Discovery on node-b
Stacks found: 12 | Locations: 2
Suggested compose_path: /mnt/user/compose

Top locations:
  /mnt/user/compose: 10 stacks
  /srv/compose: 2 stacks

Stacks:
  swag-mcp: /mnt/user/compose/swag-mcp
  syslog-mcp: /mnt/user/compose/syslog-mcp
  ...
```

Style rules:

- lead with counts and the recommended path
- list the top candidates first
- use `...` when the list continues

### Cleanup Output

Cleanup output should separate check mode from action mode.

Example:

```text
Docker Cleanup (check) on node-b
Total reclaimable: 5.2 GB (23%)

Levels:
  safe: 1.1 GB (4%)
  moderate: 3.7 GB (16%)
  aggressive: 5.2 GB (23%)

Docker Cleanup (safe) on node-b

Reclaimed:
  containers: 512 MB
  networks: 0 B
  build cache: 1.3 GB
```

Style rules:

- show reclaimable totals in check mode
- show reclaimed totals in action mode
- separate levels clearly
- keep units human-friendly and consistent

## Design Principles

### 1. Show All Data

Do not drop fields just to reduce output size. If a command returns 82 ports, the CLI should still account for all 82 ports.

### 2. Make Scanning Easy

Use:

- counts in headings
- fixed-width columns
- stable symbols
- predictable ordering
- overflow markers like `+N` and `...`

### 3. Preserve Context

Keep the important context close to the data:

- host name
- project name
- status
- counts
- requested vs returned values

### 4. Keep Patterns Consistent

If one command uses a symbol or truncation rule, reuse it everywhere.

Recommended conventions:

- `running` -> `running` or `*`
- `stopped` -> `stopped` or `-`
- success -> `yes` or `ok`
- failure -> `no` or `fail`
- overflow -> `+N`
- continuation -> `...`

## Implementation Notes

- Keep formatting in one place so CLI output stays consistent across commands.
- Prefer helpers that return rendered lines, then join them at the boundary.
- Keep command handlers thin and let services return raw data.
- Preserve the raw payload for `--json` or other structured output modes.
- Use the same summary shape for similar resources, even across different services.

## Style Improvements To Consider

These are good candidates if you want to push the style further:

1. Add a compact severity legend for outputs that mix healthy, warning, and error states.
2. Standardize table widths across commands so lists feel like part of one system.
3. Use a small set of status tokens everywhere instead of mixing words and symbols.
4. Add optional color only as a layer on top of the text layout, never as the only signal.
5. Add a `--wide` mode for users who want more columns without changing the default density.
6. Add a `--compact` mode for long lists that prioritizes scanning over detail.
7. Keep long-form raw output behind an explicit flag so the default stays readable.

## Benefits

### For CLI Users

- faster scanning
- less scrolling
- clearer status at a glance
- better grouping of related data

### For Structured Output

- full data remains available
- formatting does not affect the source payload
- downstream tools can still consume the raw result

### For Maintenance

- one formatting style across commands
- easier to add new output shapes
- fewer ad hoc formatting decisions

## Future Enhancements

- configurable verbosity levels
- optional ANSI color
- per-command compact and wide layouts
- log previews with expand-on-demand behavior
- export-friendly structured formats
