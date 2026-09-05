import test from "node:test";
import assert from "node:assert/strict";
import {closeObservations, executionAllowed, publishCurrentObservation, ScanScheduler} from "../src/orchestration.js";

test("paused or revoked permission prevents tool execution", () => {
  assert.equal(executionAllowed(true, true), false);
  assert.equal(executionAllowed(false, false), false);
  assert.equal(executionAllowed(false, true), true);
});

test("a stale scan generation cannot commit its observation", async () => {
  let current = 2;
  let commits = 0;
  const result = await publishCurrentObservation(1, () => current, async () => {}, () => { commits += 1; });
  assert.equal(result, undefined);
  assert.equal(commits, 0);
});

test("a failed discovery publish cannot commit success-like local state", async () => {
  let commits = 0;
  await assert.rejects(
    publishCurrentObservation(1, () => 1, async () => { throw new Error("offline"); }, () => { commits += 1; }),
    /offline/
  );
  assert.equal(commits, 0);
});

test("scan scheduler coalesces overlap and performs one requested rerun", async () => {
  let release;
  let passes = 0;
  const firstPass = new Promise((resolve) => { release = resolve; });
  const scheduler = new ScanScheduler(async () => {
    passes += 1;
    if (passes === 1) await firstPass;
  });
  const first = scheduler.run();
  const second = scheduler.run();
  const third = scheduler.run();
  assert.equal(first, second);
  assert.equal(second, third);
  release();
  await first;
  assert.equal(passes, 2);
});

test("scan scheduler honors its queued rerun after the current scan rejects", async () => {
  let release;
  let passes = 0;
  const firstPass = new Promise((resolve) => { release = resolve; });
  const scheduler = new ScanScheduler(async () => {
    passes += 1;
    if (passes === 1) {
      await firstPass;
      throw new Error("first scan failed");
    }
  });
  const pending = scheduler.run();
  scheduler.run();
  release();
  await assert.rejects(pending, /first scan failed/);
  assert.equal(passes, 2);
});

test("pausing attempts to close every observation and exposes failed closes", async () => {
  const closed = [];
  const results = await closeObservations([1, 2, 3], async (tabId) => {
    closed.push(tabId);
    if (tabId === 2) throw new Error("offline");
  });
  assert.deepEqual(closed, [1, 2, 3]);
  assert.deepEqual(results.map((result) => result.status), ["fulfilled", "rejected", "fulfilled"]);
});
