---
title: "Snippet performance and storage limits"
created: "2026-10-02"
updated: "2026-10-02"
---

# Snippet performance and storage limits

The [snippet performance example](../../crates/labby-codemode/examples/snippet_performance.rs) measures two separate workloads without contacting live services:

- Listing synthetic Markdown snippets with a cold metadata cache in fresh example processes, then a warm metadata cache in the parent process. The cold measurements include file reads, hashing, metadata parsing and JavaScript compilation. They exclude child-process startup and do not flush the operating system's filesystem cache.
- Executing deferred calls through `codemode.batch` in the real hardened QuickJS runner and Code Mode broker, with an in-process synthetic host. Fresh-pool samples include initial runner startup. Warm-pool samples follow explicit priming. A sequential baseline then awaits the same calls one at a time using the same warm pool and host. Each execution still creates a fresh JavaScript runtime; pooling reuses the process.

The tool catalog contains one synthetic `bench::read` tool. Its configured async delay and semaphore admission limit are part of the reported workload. The concurrency option controls that host semaphore; `codemode.batch` itself starts all deferred jobs and does not take a concurrency option. These measurements exercise the actual runtime and protocol, but do not represent upstream network latency, authentication, a production catalog, or appliance performance.

## Run

Build the product runner using the repository's supported build instructions. Supply its absolute path explicitly; the example never restarts or replaces a service.

```sh
cargo run -p labby-codemode --example snippet_performance --no-default-features --locked -- \
  --runner "$PWD/target/debug/labby" \
  --samples 10 --snippets 32 --calls 16 --concurrency 8 --latency-ms 2
```

For reproducible process reuse, explicitly select the pool policy for the invocation:

```sh
LABBY_CODE_MODE_POOL_SIZE=2 LABBY_CODE_MODE_POOL_RECYCLE_AFTER=100 \
  cargo run -p labby-codemode --example snippet_performance --no-default-features --locked -- \
  --runner "$PWD/target/debug/labby" --samples 10 --snippets 32 --calls 16 --concurrency 8 --latency-ms 2
```

`--samples` accepts 1–30, `--snippets` 1–128, `--calls` 1–64, host `--concurrency` 1–32, and `--latency-ms` 1–10. Unknown, repeated, or incomplete options fail. Each execution has a three-second deadline and the sample loops have a one-minute total budget. All source files belong to temporary directories and runner pools are explicitly drained. A disabled pool produces fresh processes for the warm series too; do not interpret that series as process reuse.

The JSON report includes every sample, median, nearest-rank p95, actual tool-call counts (including warmup), observed maximum in-flight calls, workload cardinality, build profile, pool-size override and total benchmark wall time. Total wall time includes sample process startup and warmup, but excludes source-file preparation. Small-sample p95 is descriptive: with ten samples it is the maximum sample. There are no performance threshold assertions.

Keep the report together with the source revision, runner revision/build, operating system, CPU architecture, build profile and pool environment when comparing runs. Use the same workload and build profile for before/after comparisons. A debug-build observation is not a release-build or production throughput claim.

## Cache and writer policy

The listing cache retains at most 128 entries within an 8 MiB conservative allocation charge. Individual entries over 2 MiB are not retained. Charging covers the cache container, metadata/error keys, strings, vector capacity, nested JSON defaults and conservatively estimated B-tree storage; source bodies are not retained. Cache hits refresh LRU order, while a changed name/source revision removes the prior charge before the replacement is considered. Oversized metadata cannot evict all the small entries merely to enter the cache.

Cooperating writers use the same persistent cross-process lock file. Lock acquisition retries `try_lock` in short bounded waits for up to 1.5 seconds. Contention returns a typed conflict identifying `snippet-write-lock`, says no snippet changed, and remains distinct from a stale digest conflict identifying the snippet itself. The lock stays held through digest comparison, temporary-file write, atomic publication and directory synchronization. Non-force publication additionally uses no-clobber persistence.

Focused storage tests exercise byte/entry quotas, nested defaults, invalidation, a genuine lock held by another process, timeout without writes, release/recovery and stale edit protection. The lock timeout test uses a broad termination watchdog; it is not a benchmark threshold.

## Local observation

A debug-profile run on macOS arm64 used ten samples, 32 snippets, 16 calls per execution, a 2 ms synthetic host delay, host admission of eight, and a two-process pool. Both the benchmark and the all-feature product runner were rebuilt from the final implementation sources. The complete invocation took 2.720 seconds and verified 512 actual host calls including warmup.

| Workload | Median ms | p95 ms |
| --- | ---: | ---: |
| Cold metadata listing | 5.509 | 7.320 |
| Warm metadata listing | 0.674 | 0.743 |
| Fresh-pool batch | 74.311 | 78.733 |
| Warm-pool batch | 60.826 | 60.973 |
| Warm-pool sequential | 109.206 | 109.908 |

All batch samples observed eight in-flight host calls; sequential samples observed one. These are descriptive local observations, including protocol/runtime overhead and Tokio timer scheduling, not guaranteed speedups or production latency estimates. Re-run the command against the desired product build before comparing revisions.
