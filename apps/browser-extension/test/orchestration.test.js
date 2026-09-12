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

test("same-tab events finish healthy discovery before rerunning with current policy", async () => {
  const {TabScanScheduler} = await import("../src/orchestration.js");
  const {discoverCurrentDocument} = await import("../src/scanning.js");
  const scheduler = new TabScanScheduler();
  let release;
  let started;
  const ready = new Promise(resolve => { started = resolve; });
  let injections = 0;
  const scripting = {executeScript: async options => {
    injections++;
    if (injections === 1) {
      started();
      await new Promise(resolve => { release = resolve; });
    }
    return options.world === "ISOLATED"
      ? [{documentId: "same-doc", result: {url: "https://page.example/", title: "Page"}}]
      : [{documentId: "same-doc", result: {supported: true, tools: [{name: "search"}]}}];
  }};
  let generation = 0;
  let closes = 0;
  let commits = 0;
  const policies = [];
  const requestScan = policy => {
    const ownGeneration = ++generation;
    return scheduler.run(821, async () => {
      policies.push(policy);
      const observation = await discoverCurrentDocument({id: 821, url: "https://page.example/"}, scripting, {contains: async () => true}, [], false);
      if (ownGeneration !== generation) return;
      if (!observation) { closes++; generation++; return; }
      await publishCurrentObservation(ownGeneration, () => generation, async () => {}, () => { commits++; });
    });
  };
  const first = requestScan("initial");
  await ready;
  const second = requestScan("superseded");
  const third = requestScan("latest");
  assert.equal(first, second);
  assert.equal(second, third);
  assert.equal(injections, 1);
  assert.equal(generation, 3);
  release();
  await first;
  assert.deepEqual(policies, ["initial", "latest"]);
  assert.equal(closes, 0);
  assert.equal(commits, 1);
  assert.equal(injections, 4);
  assert.equal(scheduler.entries.size, 0);
});

test("per-tab scheduling leaves other tabs independent and releases failed entries", async () => {
  const {TabScanScheduler} = await import("../src/orchestration.js");
  const scheduler = new TabScanScheduler();
  let release;
  const stalled = scheduler.run(1, () => new Promise(resolve => { release = resolve; }));
  let second = 0;
  await scheduler.run(2, async () => { second++; });
  assert.equal(second, 1);
  const failing = scheduler.run(1, async () => { throw new Error("policy read failed"); });
  release();
  await assert.rejects(failing, /policy read failed/);
  await assert.rejects(stalled, /policy read failed/);
  assert.equal(scheduler.entries.size, 0);
  await scheduler.run(1, async () => { second++; });
  assert.equal(second, 2);
});
