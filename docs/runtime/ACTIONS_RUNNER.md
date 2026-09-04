---
title: "GitHub Actions Hosted Runner Guide"
created: "2026-07-30"
updated: "2026-09-03"
---

# GitHub Actions Hosted Runner Guide

Last updated: 2026-09-03

## Runner selection

All repository-defined Linux jobs use GitHub-hosted `ubuntu-24.04` runners.
Native Windows checks use `windows-latest`. Release jobs use the native hosted
runner for each supported target.

No repository-defined workflow uses a self-hosted runner or a custom runner
label. The central fleet policy and repository contract are reusable workflow
calls owned by the organization.

## Rust cache behavior

Rust jobs use `.github/actions/setup-rust-kache/action.yml`. The action installs
the pinned Rust toolchain and Linux build dependencies, then selects the cache
path for the current hosted runner:

- Jobs with the shared MinIO credentials and a writable hosted tool cache use
  Kache.
- Jobs without those credentials use the GitHub Actions Cargo cache.
- Jobs without a usable cache run Cargo without a compiler wrapper.

The action does not depend on persistent host services or runner-local state.
Each hosted runner receives a fresh job workspace.

## Browser tests

The Gateway Admin browser job installs Chromium during the job. It installs the
required Ubuntu runtime libraries first, then verifies a real headless launch.
The browser path is `/home/runner/.cache/ms-playwright` so installation and test
execution use the same location.

## Validation

Run the same checks that CI runs before changing runner configuration:

```bash
go run github.com/rhysd/actionlint/cmd/actionlint@v1.7.7
python3 -m unittest scripts/ci/test_windows_ci_policy.py
cargo test -p labby --test ci_changed_paths --locked
git diff --check
```

Do not add a custom runner label. Use a GitHub-hosted runner label that matches
the target operating system and architecture.
