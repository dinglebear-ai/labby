# Research

Recovered the original September 25 process-extraction task and verified its live state on September 26, 2026.

The original development sandbox retained commit 7717a1b42f26d51c4cddf72622d9f65ae7aedfba, which added only a one-line Git smoke marker. The old staging sandbox served static HTML and a text health response from an earlier snapshot. Neither was a built Labby application.

The runtime is msb 0.7.3. The MCP sandbox_start path rejected it with `no tested sandbox launch contract for runtime 0.7.3`, while the native CLI successfully resumed the exact development sandbox. Existing staging remained healthy externally. No runtime or production service was replaced.

GitHub CLI authentication on the host was invalid. Existing SSH access and an explicit push dry run succeeded. The workflow therefore uses a named host SSH broker rather than putting a credential inside the VM.

The full reference package lock contains 155 installed packages on the immutable Ubuntu ARM64 base. Package availability is not indefinite; missing locked versions must fail.

Current CLI help rejects inline secret values and accepts host environment references. Snapshot state and runtime configuration must be treated separately.

Sources: live tool receipts; installed msb 0.7.3 help; [official documentation](https://docs.microsandbox.dev/llms.txt); [image behavior](https://docs.microsandbox.dev/images/disk-images).
