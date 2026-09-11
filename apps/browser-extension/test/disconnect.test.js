import test from "node:test";
import assert from "node:assert/strict";
import {readFileSync} from "node:fs";
import vm from "node:vm";
import {cancelWebMcp, invokeWebMcp} from "../src/probe.js";

// Run the actual worker handlers with Chrome boundary fakes; do not start its
// unrelated identity/scan bootstrap or maintain a second handler implementation.
function worker({
  getSettings = async () => ({}),
  setSettings = async () => {},
  removeSettings = async () => {},
  execute = async () => [],
  identityManager = {ensure: async () => ({publicKey: "fixture-key"}), revoke: async () => {}}
} = {}) {
  const listener = {addListener() {}};
  const storageWrites = [];
  const context = vm.createContext({
    console, TextEncoder, setTimeout, clearTimeout, cancelWebMcp, invokeWebMcp,
    createIdentityManager: () => identityManager, IndexedDbIdentityStore: class {},
    indexedDB: {}, crypto: {subtle: {}}, bridgeFailureKind: (error) => error instanceof Error ? error.message : String(error),
    stableStringify: JSON.stringify, canScanTab: async () => true,
    executionAllowed: () => true,
    chrome: {
      runtime: {onInstalled: listener, onStartup: listener, onMessage: listener},
      tabs: {onUpdated: listener, onActivated: listener, onRemoved: listener, get: async () => ({id: 7})},
      alarms: {onAlarm: listener}, permissions: {onAdded: listener, onRemoved: listener},
      storage: {onChanged: listener, local: {
        get: getSettings,
        set: async (value) => { storageWrites.push({type: "set", value}); await setSettings(value); },
        remove: async (value) => { storageWrites.push({type: "remove", value}); await removeSettings(value); }
      }},
      scripting: {executeScript: execute},
    },
    ScanScheduler: class { run() { return Promise.resolve(); } },
  });
  const source = readFileSync(new URL("../src/service_worker.js", import.meta.url), "utf8")
    .replace(/^import .*;\n/gm, "").replace(/\ninitialize\(\);\s*$/, "");
  vm.runInContext(`${source}\n globalThis.handlers = {executeToolCall, cancelDisconnectedCalls, resumeAndScan, reportBridgeFailure, handleUiMessage, handleServerEvent, pendingCalls, observations, setChannel(value) { channel = value; }, pairingGeneration() { return pairingGeneration; }, advancePairingGeneration() { pairingGeneration += 1; }, pairingPollActive() { return pairingPollTimer !== undefined; }, clearPairingPoll() { clearTimeout(pairingPollTimer); pairingPollTimer = undefined; }};`, context);
  context.handlers.observations.set(7, {tab_id: 7, document_id: "doc", tools: []});
  context.handlers.storageWrites = storageWrites;
  return context.handlers;
}

test("stale persisted browser id cannot promote an unauthenticated reconnect", async () => {
  const handlers = worker({getSettings: async () => ({browserId: "stale-browser"})});
  const messages = [];
  handlers.setChannel({
    browserId: undefined,
    message: async (...args) => { messages.push(args); return {payload: {}}; }
  });

  await handlers.resumeAndScan();

  assert.deepEqual(messages, []);
  assert.deepEqual(handlers.storageWrites, []);
});

test("stale pairing failure cannot delete a newer pairing association", async () => {
  const handlers = worker({getSettings: async () => ({pairingId: "new-pair"})});
  await handlers.reportBridgeFailure(new Error("pairing_not_pending"), {
    pairingId: "old-pair", pairingGeneration: handlers.pairingGeneration()
  });
  assert.deepEqual(handlers.storageWrites, []);

  handlers.advancePairingGeneration();
  await handlers.reportBridgeFailure(new Error("pairing_not_pending"), {
    pairingId: "new-pair", pairingGeneration: 0
  });
  assert.deepEqual(handlers.storageWrites, []);
});

test("connected resync cannot erase a pairing started by a newer generation", async () => {
  const settings = deferred();
  const handlers = worker({getSettings: () => settings.promise});
  handlers.setChannel({browserId: "paired-browser", message: async () => ({payload: {}})});
  const resuming = handlers.resumeAndScan();
  handlers.advancePairingGeneration();
  settings.resolve({pairingId: "new-pair"});
  await resuming;
  assert.deepEqual(handlers.storageWrites, []);
});

test("failed replacement pairing restores polling for the prior association", async () => {
  const handlers = worker({getSettings: async () => ({pairingId: "prior-pair"})});
  handlers.setChannel({browserId: undefined, message: async (type) => {
    if (type === "pairing.request") throw new Error("transient_pairing_failure");
    if (type === "pairing.status") return {payload: {status: "pending", pairing_id: "prior-pair", expires_at: Math.floor(Date.now() / 1000) + 60}};
    return {payload: {}};
  }});
  await assert.rejects(handlers.handleUiMessage({type: "pair"}), /transient_pairing_failure/);
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(handlers.pairingGeneration(), 2);
  assert.equal(handlers.pairingPollActive(), true);
  handlers.clearPairingPoll();
});

test("approval from an older pairing generation cannot install browser state", async () => {
  const handlers = worker({getSettings: async () => ({pairingId: "new-pair"})});
  let closed = false;
  handlers.setChannel({browserId: undefined, close() { closed = true; }});
  handlers.advancePairingGeneration();
  await handlers.handleServerEvent(
    {type: "pairing.approved", payload: {browser_id: "stale-browser", pairing_id: "old-pair"}},
    undefined,
    0
  );
  assert.equal(closed, false);
  assert.deepEqual(handlers.storageWrites, []);
});

test("auth failure closes the stale channel before asynchronous identity revocation", async () => {
  const gate = deferred();
  let revoked = false;
  const handlers = worker({identityManager: {revoke: async () => { revoked = true; await gate.promise; }}});
  let closed = false;
  const staleChannel = {browserId: "stale", close() { closed = true; }};
  handlers.setChannel(staleChannel);
  const reporting = handlers.reportBridgeFailure(new Error("auth_failed"), {});
  assert.equal(closed, true);
  assert.equal(staleChannel.browserId, undefined);
  assert.equal(revoked, true);
  gate.resolve();
  await reporting;
});

const payload = {call_id: "call", tab_id: 7, document_id: "doc", catalog_fingerprint: "[]", tool_name: "tool"};
const deferred = () => { let resolve; const promise = new Promise((r) => { resolve = r; }); return {promise, resolve}; };

test("disconnect during permission wait cancels exact document and prevents page execution", async () => {
  const settings = deferred();
  const injections = [];
  const handlers = worker({getSettings: () => settings.promise, execute: async (request) => { injections.push(request); return []; }});
  let current = true;
  const replies = [];
  const connection = {isCurrent: () => current, messageNow: async (...args) => replies.push(args)};
  const running = handlers.executeToolCall(payload, connection);
  current = false;
  await handlers.cancelDisconnectedCalls(connection);
  settings.resolve({});
  await running;
  assert.equal(injections.length, 1);
  assert.equal(injections[0].func, cancelWebMcp);
  assert.equal(injections[0].target.tabId, 7);
  assert.deepEqual(Array.from(injections[0].target.documentIds), ["doc"]);
  assert.deepEqual(Array.from(injections[0].args), ["call"]);
  assert.equal(replies.length, 0);
});

test("late old execution cannot deliver to or delete a replacement generation call", async () => {
  const execution = deferred();
  const entered = deferred();
  const handlers = worker({execute: async (request) => {
    if (request.func === invokeWebMcp) { entered.resolve(); return execution.promise; }
    return [];
  }});
  let current = true;
  const replies = [];
  const connection = {isCurrent: () => current, messageNow: async (...args) => replies.push(args)};
  const running = handlers.executeToolCall(payload, connection);
  await entered.promise;
  current = false;
  await handlers.cancelDisconnectedCalls(connection);
  const replacement = {connection: {}, tab_id: 8, document_id: "new", cancelled: false};
  handlers.pendingCalls.set("call", replacement);
  await handlers.cancelDisconnectedCalls(connection);
  assert.equal(replacement.cancelled, false);
  execution.resolve([{result: {__webby_execution_v1__: true, ok: true, value: "old"}}]);
  await running;
  assert.equal(handlers.pendingCalls.get("call"), replacement);
  assert.equal(replies.length, 0);
});

test("disconnect attempts every cancellation and exposes injection failures", async () => {
  const attempted = [];
  const handlers = worker({execute: async (request) => {
    attempted.push(request.args[0]);
    if (request.args[0] === "first") throw new Error("injection_failed");
    return [];
  }});
  const connection = {isCurrent: () => false, messageNow: async () => {}};
  const first = {connection, tab_id: 7, document_id: "doc", cancelled: false};
  const second = {...first};
  handlers.pendingCalls.set("first", first);
  handlers.pendingCalls.set("second", second);
  await assert.rejects(handlers.cancelDisconnectedCalls(connection), /disconnect_cancellation_failed/);
  assert.deepEqual(attempted, ["first", "second"]);
  assert.equal(first.cancelled, true);
  assert.equal(second.cancelled, true);
  assert.equal(handlers.pendingCalls.size, 0);
});
