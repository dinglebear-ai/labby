# First-use release qualification

`scripts/ci/qualify-first-use.py` is a release qualification harness for disposable native machines. It measures from before installer download through authenticated first-use evidence. It is not an installer for an existing workstation, and its unit fixtures do not qualify a release.

## Supported starting conditions

Use a fresh disposable macOS arm64 or supported Linux machine with cold caches, network access, native service installation privileges, and an already usable supported Agent provider account. Record account creation, billing approval, provider outages, OAuth interaction, and external-client login separately when those conditions apply. Do not silently exclude those failures from completion rates.

The harness requires Python 3.9 or newer and the native installer's platform prerequisites. It uses the reviewed shared verifier bootstrap to reuse GitHub CLI `2.102.0` or newer, or download and verify pinned verifier bytes in a private directory. No developer account or preinstalled GitHub CLI is required. This bootstrap trusts the checked-in harness/helper/pins; the product's initial canonical HTTPS installer trust is explained in [Standalone installer bootstrap trust](../plans/STANDALONE_INSTALLER_TRUST.md). A passing automated run does not establish browser interaction completion or a real five-minute release result by itself.

The `--disposable-machine` argument authorizes native service and client configuration changes by the released installer and reviewed test drivers. Never run this against a development workstation or production server. State directories and the install target must be absent. Native service roots must match the real platform defaults: the invoking user's `.labby`, and `/home/labby/.labby` for the Linux service. The measured gateway must be local.

## Running a qualification

Provide a private JSON plan, an immutable release tag, and a new report path:

```sh
python3 scripts/ci/qualify-first-use.py \
  --disposable-machine \
  --tag vX.Y.Z \
  --plan /absolute/path/first-use-plan.json \
  --report /absolute/path/first-use-report.json
```

The default timing budget is 300 seconds. The 600-second execution deadline permits recording a completed but slow run as a timing failure. Increasing the deadline does not increase the five-minute budget.

The plan has these fields:

| Field | Meaning |
| --- | --- |
| `conditions` | `fresh_machine`, `cold_caches`, `provider_access_ready`, `native_service`, `public_catalog`, `provider_cost_accepted`, all `true`; verified by the disposable image/runner owner. Selected clients additionally require `external_client_auth_ready` and `external_client_model_ready` both `true`. |
| `state_root` | Actual native service state directory, absent before the run. |
| `invoking_state_root` | Actual invoking user's `.labby`, absent before the run. |
| `install_dir` | New installation target, absent before the run. |
| `gateway_url` | Local gateway origin, without credentials, path, query, or fragment. |
| `token_file` | New private raw-token file created by the authenticated bootstrap driver; owned by the invoking user, mode `0600`. |
| `selected_clients` | Explicit list of `codex` and/or `claude-code`, or `[]` for Labby only. |
| `stages` | Optional reviewed overrides for exactly `bootstrap`, `agent`, `clients`, `discover`, `mcp`; each is an argv array. Omit to use the checked-in native driver. No shell-string interpretation. |

Driver integration is explicit because provider login and client interaction differ across supported images. The checked-in `scripts/ci/first-use-native-driver.py` provides the native bearer, Labby-only path; omit `stages` to use it. Selected-client variants register the protected bridge and gateway-issued observation sessions, then invoke actual supported Codex/Claude Code CLI processes after MCP approval. Registration alone never earns completion. No end-to-end run is claimed. The standard driver uses only binary-owned setup/settings/Agent/gateway operations. Override drivers must operate the released product's CLI/browser/API operations; they must not write readiness journals, mock providers/catalogs/MCP, seed success responses, bypass authentication, or manually edit `.env`/TOML to finish onboarding.

The installer runs with `LABBY_INSTALL_NO_SETUP=1`, followed by the bootstrap driver using the actual activated binary. Each driver receives:

- `LABBY_QUALIFICATION_BINARY`: exact newly activated release binary.
- `LABBY_QUALIFICATION_GATEWAY_URL`: local test gateway.
- `LABBY_QUALIFICATION_TOKEN_FILE`: private authenticated token output path.
- `LABBY_QUALIFICATION_SELECTED_CLIENTS`: JSON selected-client list.

The standard driver requires protected runner inputs `LABBY_QUALIFICATION_PROVIDER_URL`, `LABBY_QUALIFICATION_PROVIDER_KEY` (optional for an explicitly unauthenticated provider), and `LABBY_QUALIFICATION_MODEL`. It discovers real model IDs and uses authenticated personal ownership from `/auth/session`, not a copied owner ID. The bounded Agent test may incur provider charges.

Approve an existing public catalog selection before starting: `LABBY_QUALIFICATION_QUERY`, `LABBY_QUALIFICATION_ARTIFACT_ID`, `LABBY_QUALIFICATION_REVISION_ID`, `LABBY_QUALIFICATION_MCP_URL`, and `LABBY_QUALIFICATION_TOOL`. Optional `LABBY_QUALIFICATION_TOOL_ARGUMENTS` is a JSON object; `LABBY_QUALIFICATION_MCP_TOKEN` is required for bearer metadata. The driver requires exact metadata/revision/endpoint matches, enables tools only, and invokes the backend's eligible-tool verification operation with explicit approval. No generic README commands are executed. The gateway validates tool schema and read-only/non-destructive eligibility.

The native driver explicitly saves `LABBY_AGENT_PROVIDER_PROTOCOL=openai`. Set `LABBY_QUALIFICATION_PROVIDER_PROTOCOL=phoenix` only when qualifying a Phoenix session-extension provider; unknown modes fail before writing settings. Readiness and Agent revision harness fingerprints bind this protocol choice.

For selected clients, supply `LABBY_QUALIFICATION_CODEX_MODEL` and/or `LABBY_QUALIFICATION_CLAUDE_MODEL` for already authenticated installed clients. The driver checks their installed help output before using reviewed noninteractive flags. It discovers the actual gateway MCP tool listing without a client-observation header, requires the approved read-only/non-destructive tool to be uniquely advertised, and limits the client invocation to that named tool and scalar arguments. It never directly invokes the bridge while pretending to be an external application. Codex uses read-only sandboxing, disables shell execution, inventories configured MCP servers, temporarily disables every server except Labby, and limits Labby to the approved tool. These overrides do not edit the user's client configuration. Claude disables built-in tools, allowlists the approved MCP tool, denies additional permission prompts, and caps API spending at one dollar. Each client process has a 60-second timeout, and gateway-observed completion is required afterward. Those additional model calls may incur charges; `provider_cost_accepted` covers the Agent and selected-client tests.

Flags were checked against installed CLI help and the official [Codex reference](https://developers.openai.com/codex/cli/reference/) and [Claude Code reference](https://code.claude.com/docs/en/cli-reference). Client versions/auth/model readiness belong to the disposable image qualification matrix, not an unrecorded prerequisite.


Provider credentials come from the runner's protected secret mechanism. Do not put secrets in plans or argv. Drivers must create a real provider/model check and bounded Agent completion, explicitly select or defer clients, obtain real results from the default Public Depot, approve an eligible MCP server, and perform its safe tool verification. Registered configuration alone is not external-client connection proof.

## What the harness verifies

1. Downloads the released installer, checksum, and public attestation bundle from the explicit tag. The clock already runs.
2. Checks the installer digest and GitHub provenance against `github.com`, the requested repository, `release.yml`, the exact tag ref, and hosted runners. Empty/private GitHub configuration and removal of GitHub tokens enforce the account-free bundle path.
3. Runs that verified installer with source fallback, local-candidate, rollback, and recovery escapes disabled. The installer independently verifies the binary release.
4. Verifies the activated binary's digest against its protected activation receipt, `source=release`, and exact resolved tag.
5. Runs all reviewed stage drivers with a common deadline; timeout terminates their process group.
6. Reads `/v1/setup` `readiness.state` through authenticated, redirect-free HTTP. Every required proof must be fresh within this run. Catalog proof must name `public`. Selected clients cannot pass using deferral; deferral is allowed only for an explicit empty client selection.

A `ready: true` alone does not pass the harness. Missing, repeated, pending, stale, future-dated, or resource-free proofs fail it. Provider/MCP output, bearer credentials, private resource IDs, and driver argv are omitted from reports.

## Reporting and release gate

Every run produces a private JSON report identifying `journey: native_api_cli` and `browser_ui_qualified: false`, with individual stage durations/statuses, completion, elapsed time, timing-budget result, prerequisites, and supported conditions. Preflight failures also produce a failure report. Do not overwrite reports or retry until successful while discarding failed attempts.

Qualify actual published artifacts on each supported clean image with cold caches and real external systems. Retain every attempt. Publish completion rate, failure causes, and timing distribution, grouped by platform, release tag, selected clients, provider condition, and image conditions. A run that completes after 300 seconds has `completion: true` and `within_budget: false`; a fast partial installation has both `false`.

Focused harness tests:

```sh
python3 scripts/ci/test_first_use_qualification.py
python3 scripts/ci/test_first_use_native_driver.py
python3 -m py_compile scripts/ci/qualify-first-use.py scripts/ci/first-use-native-driver.py
```

These contract tests use synthetic evidence and receipts. They test rejection/reporting behavior only. No clean-machine first-use result, catalog availability, real Agent completion, MCP invocation, or five-minute release claim is established by them.
