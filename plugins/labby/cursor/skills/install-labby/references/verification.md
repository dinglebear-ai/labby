# Verification

Read authenticated `setup.readiness.state` through the supported binary/API operation. Require current evidence for authenticated running Labby, the provider's actual models, a completed starter Agent run, real use by every selected external client, a complete catalog search under its configured access policy, and an approved MCP server's exposed tool completing successfully. Configuration files, daemon health, tool listings, and successful artifact imports do not prove those steps.

Check persistence and use `labby doctor` to diagnose failures. Optional diagnostic MCP clients and Code Mode checks may help advanced deployments; they are not prerequisites for normal first use. Defer external applications only when the user chooses Labby-only use; skipping required Agent, catalog, or MCP checks cannot produce full readiness. Keep failures visible and resumable.

Report exact evidence, deployment target, URLs, and authentication topology without secrets. Fixture tests and browser mocks do not establish released-artifact or production readiness. A five-minute result requires a timed cold-machine release run with real external services and all required evidence.
