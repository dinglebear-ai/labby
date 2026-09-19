#!/usr/bin/env bash
# Run one shard of the workspace test suite for the `test` job in ci.yml.
#
# Shards select a small Cargo target set first: nextest compiles every selected
# target before applying test filters, so a hash partition over the whole
# workspace would still link all of `crates/labby/tests` on every runner. The
# integration shards apply one additional filter only to de-duplicate shared
# support-harness tests after the target set is bounded.
set -euo pipefail

shard="${1:-}"
labby_integration_shards=5
unit_shards=4

usage() {
  echo "usage: $0 <unit-1..${unit_shards}|labby-int-1..${labby_integration_shards}|crates>" >&2
  exit 64
}

[ -n "$shard" ] || usage
common=(--all-features --locked --profile ci)

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
    cargo nextest run --workspace "${common[@]}" --lib --bins \
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
    cargo nextest run -p labby "${common[@]}" "${targets[@]}" \
      -E 'not test(live_labby::) | binary(=live_process_harness)'
    ;;
  crates)
    # Integration tests of the extracted crates, then the workspace doctests
    # that used to run inside the blocking Rustdoc job.
    cargo nextest run --workspace --exclude labby "${common[@]}" --test '*'
    RUSTDOCFLAGS="${RUSTDOCFLAGS:-} -D warnings" \
      cargo test --doc --workspace --all-features --locked
    ;;
  *)
    usage
    ;;
esac
