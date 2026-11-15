#!/usr/bin/env bash
# Run one shard of the workspace or gateway-only test suite.
#
# `archive` (CI's all-features suite) executes a prebuilt nextest archive from
# NEXTEST_ARCHIVE and compiles nothing, so every shard can hash-partition the
# whole test set. It extracts into the checkout so compile-time paths such as
# CARGO_BIN_EXE_* and CARGO_MANIFEST_DIR stay valid.
#
# The compiling suites (`all-features`, `gateway-only`) select a small Cargo
# target set first: nextest compiles every selected target before applying
# test filters, so a hash partition over the whole workspace would still link
# all of `crates/labby/tests` on every runner.
#
# Every integration selection de-duplicates the shared support-harness tests:
# `support/live_labby.rs` is embedded into many integration binaries, so its
# internal harness tests run only in the dedicated live_process_harness target.
set -euo pipefail

shard="${1:-}"
suite="${2:-all-features}"
labby_integration_shards=5
unit_shards=4

harness_dedup='not test(live_labby::) | binary(=live_process_harness)'

usage() {
  echo "usage: $0 <unit-1..${unit_shards}|labby-int-1..${labby_integration_shards}|crates|migrations> [all-features|gateway-only|archive]" >&2
  echo "       $0 doctests" >&2
  exit 64
}

shard_index() {
  local index="$1" count="$2"
  case "$index" in
    ''|*[!0-9]*) usage ;;
  esac
  if [ "$index" -lt 1 ] || [ "$index" -gt "$count" ]; then
    usage
  fi
  printf '%s\n' "$index"
}

[ -n "$shard" ] || usage

if [ "$shard" = doctests ]; then
  # Rustdoc's strict doc build gates separately; this executes the examples.
  RUSTDOCFLAGS="${RUSTDOCFLAGS:-} -D warnings" \
    cargo test --doc --workspace --all-features --locked
  exit 0
fi

if [ "$suite" = archive ]; then
  archive="${NEXTEST_ARCHIVE:-}"
  [ -f "$archive" ] || { echo "NEXTEST_ARCHIVE must name the nextest archive file (got '${archive}')" >&2; exit 64; }
  run_archive() {
    cargo nextest run --archive-file "$archive" --workspace-remap . \
      --extract-to . --extract-overwrite --profile ci "$@"
  }
  case "$shard" in
    unit-*)
      index="$(shard_index "${shard#unit-}" "$unit_shards")"
      run_archive -E 'kind(lib) | kind(proc-macro) | kind(bin)' \
        --partition "hash:${index}/${unit_shards}"
      ;;
    labby-int-*)
      index="$(shard_index "${shard#labby-int-}" "$labby_integration_shards")"
      run_archive -E "package(labby) & kind(test) & (${harness_dedup})" \
        --partition "hash:${index}/${labby_integration_shards}"
      ;;
    crates)
      run_archive -E 'not package(labby) & kind(test)'
      ;;
    migrations)
      run_archive -E 'package(labby) & kind(lib) & test(access::migrations::)'
      ;;
    *)
      usage
      ;;
  esac
  exit 0
fi

case "$suite" in
  all-features)
    common=(--all-features --locked --profile ci)
    unit_packages=(--workspace)
    ;;
  gateway-only)
    common=(--no-default-features --features "gateway,proxy-testkit" --locked --profile ci)
    unit_packages=(-p labby)
    ;;
  *) usage ;;
esac

case "$shard" in
  unit-*)
    # Library and binary unit-test harnesses for every crate, split by test
    # hash. The `labby` lib harness is the single largest link in the
    # workspace, so every slice pays for it once and then runs its share.
    index="${shard#unit-}"
    case "$index" in
      ''|*[!0-9]*) usage ;;
    esac
    if [ "$index" -lt 1 ] || [ "$index" -gt "$unit_shards" ]; then
      usage
    fi
    cargo nextest run "${unit_packages[@]}" "${common[@]}" --lib --bins \
      --partition "hash:${index}/${unit_shards}"
    ;;
  labby-int-*)
    # `crates/labby/tests/*.rs` are 60 separate binaries that each link the
    # whole product library. Assign them round-robin by sorted position so a
    # new file automatically lands in a shard without touching ci.yml.
    index="${shard#labby-int-}"
    case "$index" in
      ''|*[!0-9]*) usage ;;
    esac
    if [ "$index" -lt 1 ] || [ "$index" -gt "$labby_integration_shards" ]; then
      usage
    fi
    targets=()
    position=0
    for file in crates/labby/tests/*.rs; do
      position=$((position + 1))
      if [ $((position % labby_integration_shards)) -eq $((index % labby_integration_shards)) ]; then
        name="$(basename "$file" .rs)"
        targets+=(--test "$name")
      fi
    done
    [ "${#targets[@]}" -gt 0 ] || { echo "shard $shard selected no targets" >&2; exit 1; }
    # `support/live_labby.rs` is embedded into many integration binaries, so its
    # internal harness tests would otherwise run once per binary. Execute those
    # shared tests only in the dedicated live_process_harness target; every
    # binary's actual product tests still run normally. This removes duplicate
    # process supervisors and keeps the shard fast without weakening coverage.
    cargo nextest run -p labby "${common[@]}" "${targets[@]}" -E "$harness_dedup"
    ;;
  crates)
    [ "$suite" = all-features ] || usage
    # Integration tests of the extracted crates. Doctests are `doctests`.
    cargo nextest run --workspace --exclude labby "${common[@]}" --test '*'
    ;;
  *)
    usage
    ;;
esac
