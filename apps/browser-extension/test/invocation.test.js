import test from "node:test";
import assert from "node:assert/strict";
import {cancelWebMcp, invokeWebMcp} from "../src/probe.js";
import {normalizeTools, stableStringify} from "../src/scanning.js";

test("executes only the named tool from the expected catalog", async () => {
  const tool = {name: "find", description: "Find", inputSchema: {type: "object"}};
  globalThis.document = {modelContext: {
    getTools: async () => [tool],
    executeTool: async (selected, input) => {
      assert.equal(selected, tool);
      assert.deepEqual(JSON.parse(input), {query: "hello"});
      return '{"count":1}';
    }
  }};

  const catalog = stableStringify(normalizeTools([tool]));
  assert.deepEqual(await invokeWebMcp("find", {query: "hello"}, "call-1", catalog), {count: 1});
  delete globalThis.document;
});

test("aborts the exact pending page call", async () => {
  const tool = {name: "wait", description: "Wait", inputSchema: {}};
  globalThis.document = {modelContext: {
    getTools: async () => [tool],
    executeTool: (_tool, _input, {signal}) => new Promise((_resolve, reject) => {
      signal.addEventListener("abort", () => reject(new Error("aborted")));
    })
  }};

  const catalog = stableStringify(normalizeTools([tool]));
  const pending = invokeWebMcp("wait", {}, "call-2", catalog);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(cancelWebMcp("call-2"), true);
  await assert.rejects(pending, /aborted/);
  // Cancellation is idempotent and leaves a tombstone so a late execution
  // carrying the same call id cannot start after cancellation was acknowledged.
  assert.equal(cancelWebMcp("call-2"), true);
  delete globalThis.document;
});

test("cancellation while getTools is pending prevents executeTool", async () => {
  let release;
  const toolsReady = new Promise((resolve) => { release = resolve; });
  let executions = 0;
  const tool = {name: "mutate", inputSchema: {}};
  globalThis.document = {modelContext: {
    getTools: () => toolsReady,
    executeTool: async () => { executions += 1; }
  }};

  const pending = invokeWebMcp("mutate", {}, "call-before-tools", stableStringify(normalizeTools([tool])));
  assert.equal(cancelWebMcp("call-before-tools"), true);
  release([tool]);
  await assert.rejects(pending, /AbortError/);
  assert.equal(executions, 0);
  delete globalThis.document;
});

test("a cancellation tombstone prevents a later invocation from starting", async () => {
  let getToolsCalls = 0;
  globalThis.document = {modelContext: {
    getTools: async () => { getToolsCalls += 1; return []; },
    executeTool: async () => { throw new Error("must not execute"); }
  }};
  cancelWebMcp("late-call");
  await assert.rejects(invokeWebMcp("anything", {}, "late-call", "[]"), /AbortError/);
  assert.equal(getToolsCalls, 0);
  delete globalThis.document;
});
