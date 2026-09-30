---
title: "GitHub Actions Hosted Runner Guide"
created: "2026-07-30"
updated: "2026-09-03"
---

# GitHub Actions Hosted Runner Guide

Last updated: 2026-09-03

## Runner selection

All repository-defined Linux jobs use GitHub-hosted `ubuntu-24.04` runners.
Native Windows checks use `windows-latest` and require a manual dispatch with
`run_windows=true`. Release jobs use the native hosted
runner for each supported target.

No repository-defined workflow uses a self-hosted runner or a custom runner
label. Fleet policy and the repository contract execute in local hosted jobs;
the latter checks out an immutable organization-owned checker implementation.

## Rust cache behavior

Rust jobs use `.github/actions/setup-rust-kache/action.yml`. The action installs
the pinned Rust toolchain and Linux build dependencies, then selects the cache
path for the current hosted runner:

- Repository workflows deliberately clear shared MinIO credentials and use
  the GitHub Actions Cargo cache on Linux, with no compiler wrapper.
- The composite retains a conditional Kache path requiring credentials,
  writable tool cache, and `KACHE_S3_PREFIX_ENFORCED=true`; repository secrets
  must not enable that path for branch-controlled PR jobs.
- Disabled caching and non-Linux callers clear the compiler wrapper; individual
  jobs may configure their own Cargo cache.

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
