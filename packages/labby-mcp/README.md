# labby-mcp

Node launcher for the Labby Rust binary.

This package makes Labby launchable from MCP clients that expect an `npx`
command. The npm package downloads the matching prebuilt binary from GitHub
Releases during install and forwards all arguments to it.

```bash
npx -y labby-mcp mcp
```

For a global install:

```bash
npm install -g labby-mcp
labby mcp
```

The MCP Registry identity for this package is:

```text
ai.dinglebear/labby
```

## Environment

- `LABBY_VERSION` or `LABBY_BINARY_VERSION`: override the GitHub release tag.
- `LABBY_REPO`: override the GitHub repository that hosts release assets.
- `LABBY_RELEASE_BASE_URL`: override the full release download base URL.
- `LABBY_SKIP_DOWNLOAD=1`: skip download during install.

## Supported Platforms

- Linux x64
- Windows x64

macOS is not advertised until the release workflow publishes a macOS binary.
