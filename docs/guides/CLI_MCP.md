# Serve selected CLI tools as MCP

On Linux, `labby cli serve` runs a standalone MCP server over stdio. It needs
neither a Labby daemon nor access to Labby's credentials. A standard MCP
client may launch it directly. Register it with a Labby gateway as a regular
stdio upstream to use the normal admin, authorization, exposure, and Code Mode
surfaces. It is not a Labby virtual server and does not advertise resources or
prompts.

## Describe the tools

Place a manifest in an operator-managed directory outside credential and user
homes, for example `/var/lib/labby-upstreams/my-cli/tools.toml`. Confirm the
listed executable and every configured invocation really are read-only. The
adapter does not prove the intent of a binary. Only install native application
executables you trust, not process launchers, interpreters, or daemons.

```toml
[[tools]]
name = "hello"
description = "Print one of the approved greetings"
executable = "/usr/bin/printf"
working_directory = "/"
args = ["%s"]
read_only = true

[tools.choices]
world = ["Hello world"]
labby = ["Hello Labby"]
```

Without a `choices` table the tool takes no input and runs the fixed arguments.
With choices, the tool takes one enum field and appends the configured argument
sequence for that value. The caller cannot supply an executable, args, shell,
environment or working directory. The server accepts at most two concurrent
calls, caps both output pipes and runs each tool without ambient environment
or stdin, under a ten-second deadline. Unknown options and requested write
capabilities are rejected rather than accepted as generic commands.

## Add it to the gateway

Use the installed Labby binary's exact canonical path. A gateway administrator
must approve that path under the ordinary spawn policy. Do not disable the
guard or allow all paths with the same filename.

```toml
[gateway]
extra_stdio_commands = ["/usr/local/bin/labby"]
```

Then use Add Server with that command and these four arguments: `cli`, `serve`,
`--manifest`, and the absolute manifest path. The equivalent CLI is:

```bash
labby server add my-cli --command /usr/local/bin/labby \
  --arg cli --arg serve --arg=--manifest --arg /var/lib/labby-upstreams/my-cli/tools.toml
labby server test my-cli --json
labby code search 'my-cli hello'
```

This creates the same upstream as [any configured stdio MCP server](../services/UPSTREAM.md#stdio-upstream).
For other data paths, configure only the explicit read-only sandbox grants
the command requires. Standard MCP listing and invocation use the same
gateway scopes and visibility as other upstreams. If Code Mode hides raw
upstream tools from the root MCP listing, discover and invoke them with
Code Mode or with an appropriately scoped MCP route.
