# Changelog: Labby plugin

## Unreleased

### Added

- Added the first-class `install-labby` Agent Skill with guided verified
  release installation, authentication selection, listener/deployment
  configuration, Incus guidance, Tailscale Funnel, approval-gated reverse-proxy
  configuration, agent MCP onboarding, first-party live MCP verification, and
  optional Phoenix/Codex App Server setup.
- Added focused references for OAuth provider setup, Incus/agent isolation,
  public exposure, reverse-proxy safety, and runtime verification.
- Added an OpenAI skill descriptor so `$install-labby` has an explicit
  Codex-facing prompt.
- Added the `LABBY_SETUP_AUTH=bearer|oauth|both` installer handoff and the
  matching `labby setup --auth` selector.

### Changed

- Replaced the install Skill's mutable third-party package-runner verification with a
  checked-in, protocol-explicit HTTP MCP verifier that discovers the live
  `gateway` tool and executes the read-only `gateway.help` action.
- The distributable plugin now defaults to Labby's ordinary loopback endpoint,
  `http://127.0.0.1:8765`, rather than a private deployment-specific proxy.
- Documented the bundled MCP entry as bearer-specific and added the explicit
  higher-precedence native-client path required for OAuth-oriented Claude Code
  connections.
- OAuth metadata now reflects Google **or** Authelia and documents that a
  static bearer can coexist with OAuth as a break-glass credential.
- Existing `--oauth google|authelia` server setup remains backwards
  compatible and resolves to OAuth + bearer when `--auth` is omitted.
- Switching server setup back to bearer-only authentication now clears stale
  OAuth provider, bootstrap-admin, and provider client credential fields from
  the protected environment.
- Plugin installation guidance now points to the verified release installer
  and `$install-labby` rather than the retired raw-branch curl pipeline.

### Removed

- Removed stale changelog guidance claiming SessionStart / ConfigChange hooks
  still perform setup synchronization. The plugin ships no automatic hooks.
- Removed public plugin documentation that depended on the private Dookie
  deployment.

### Historical packaging note

The Labby plugin no longer bundles the `labby` binary. The package contains
Agent Skills, MCP/plugin metadata, and documentation. Binary installation is an
explicit verified release operation.
