import test from "node:test";
import assert from "node:assert/strict";
import {LabbyBrowserChannel} from "../src/channel.js";

function channel(options = {}) {
  const instance = new LabbyBrowserChannel({baseUrl: "http://127.0.0.1:8765", extensionId: "a".repeat(32), ...options});
  const frames = [];
  instance.socket = {readyState: 1, send(value) { frames.push(JSON.parse(value)); }};
  instance.connection = {isCurrent: () => true, messageNow: (...args) => instance.messageNow(...args)};
  globalThis.WebSocket = {OPEN: 1};
  return {instance, frames};
}

test("sends versioned plain JSON and correlates replies", async () => {
  const {instance, frames} = channel();
  const pending = instance.messageNow("pairing.request", {display_name: "Chrome", public_key: "key"});
  assert.equal(frames[0].version, 1);
  assert.equal(frames[0].type, "pairing_request");
  assert.equal(frames[0].extension_id, "a".repeat(32));
  instance.receive({version: 1, request_id: frames[0].request_id, type: "pairing_pending", pairing_id: "pair", expires_at: 1, pairing_fingerprint: "A1B2C3D4E5F6"});
  const reply = await pending;
  assert.equal(reply.payload.pairing_id, "pair");
  assert.equal(reply.payload.pairing_fingerprint, "A1B2C3D4E5F6");
});

test("maps Rust tool calls to extension events", async () => {
  let event;
  const {instance} = channel({onEvent(value) { event = value; }});
  instance.receive({version: 1, type: "tool_call", call_id: "call", tab_id: 1, document_id: "doc", catalog_revision: 2, tool_name: "search", arguments: {}});
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(event.type, "tool.call");
  assert.equal(event.payload.call_id, "call");
});

test("keeps the MV3 service worker alive with acknowledged protocol heartbeats", async () => {
  const sockets = installSocket();
  const instance = new LabbyBrowserChannel({baseUrl: "http://localhost:8765", extensionId: "id", onChallenge() {}, onError() {}, heartbeatIntervalMs: 5});
  instance.connect();
  await sockets[0].onopen();
  await new Promise((resolve) => setTimeout(resolve, 16));
  const heartbeats = () => sockets[0].frames.filter((frame) => frame.type === "heartbeat");
  const sentBeforeClose = heartbeats().length;
  assert.ok(sentBeforeClose >= 2);
  assert.ok(heartbeats().every((frame) => frame.version === 1 && typeof frame.request_id === "string"));
  for (const heartbeat of heartbeats()) {
    instance.receive({version: 1, request_id: heartbeat.request_id, type: "acknowledged", received: "heartbeat"});
  }
  assert.equal(instance.pending.size, 0);
  instance.close();
  await new Promise((resolve) => setTimeout(resolve, 12));
  assert.equal(heartbeats().length, sentBeforeClose);
});

test("reconnects when heartbeat acknowledgements stop", async () => {
  const sockets = installSocket();
  const failures = [];
  const instance = new LabbyBrowserChannel({
    baseUrl: "http://localhost:8765", extensionId: "id", onChallenge() {},
    onError(_error, context) { failures.push(context?.kind); },
    heartbeatIntervalMs: 5, replyTimeoutMs: 10
  });
  instance.connect();
  await sockets[0].onopen();
  await new Promise((resolve) => setTimeout(resolve, 40));
  assert.equal(sockets[0].closed, true);
  assert.ok(failures.includes("heartbeat_failed"));
  instance.close();
});

test("disconnect cancellation is once-only and bound to the closing socket", () => {
  const sockets = installSocket();
  const disconnected = [];
  const instance = new LabbyBrowserChannel({baseUrl: "http://localhost:8765", extensionId: "id", onChallenge() {}, onError() {}, onDisconnect(connection) { disconnected.push(connection); }});
  instance.connect();
  const first = instance.connection;
  const staleClose = sockets[0].onclose;
  instance.connect();
  assert.deepEqual(disconnected, [first]);
  assert.equal(first.isCurrent(), false);
  const second = instance.connection;
  staleClose();
  assert.equal(second.isCurrent(), true);
  assert.equal(disconnected.length, 1);
  sockets[1].onclose();
  assert.deepEqual(disconnected, [first, second]);
  assert.equal(second.isCurrent(), false);
  instance.close();
  assert.equal(disconnected.length, 2);
});

test("publishes sanitized observations with a stable positive catalog revision", async () => {
  const {instance, frames} = channel();
  const pending = instance.messageNow("discovery.observed", {observations: [{url: "https://example.com/path?secret=yes", title: "Example", tab_id: 7, document_id: "doc", tools: [{name: "search"}]}]});
  assert.equal(frames[0].origin, "https://example.com");
  assert.equal(frames[0].sanitized_path, "/path");
  assert.ok(frames[0].catalog_revision > 0);
  instance.receive({version: 1, request_id: frames[0].request_id, type: "acknowledged", received: "observe"});
  await pending;
});

test("reply deadlines remove pending requests", async () => {
  const {instance} = channel({replyTimeoutMs: 5});
  await assert.rejects(instance.messageNow("pairing.status", {pairing_id: "missing"}), /channel_reply_timeout/);
  assert.equal(instance.pending.size, 0);
});

function installSocket() {
  const sockets = [];
  globalThis.WebSocket = class {
    static OPEN = 1;
    readyState = 1;
    frames = [];
    send(value) { this.frames.push(JSON.parse(value)); }
    constructor() { sockets.push(this); }
    close() { this.closed = true; this.onclose?.(); }
  };
  return sockets;
}

test("closing before open rejects callers waiting for readiness", async () => {
  installSocket();
  const instance = new LabbyBrowserChannel({baseUrl: "http://127.0.0.1:8765", extensionId: "a".repeat(32), onChallenge() {}, onError() {}});
  instance.connect();
  const pending = instance.message("pairing.status", {pairing_id: "pending"});
  instance.close();
  await assert.rejects(pending, /channel_closed/);
});

test("an authentication capability cannot send an old challenge after reconnect", async () => {
  const sockets = installSocket();
  let resumeSigning;
  const signing = new Promise((resolve) => { resumeSigning = resolve; });
  let signingStarted;
  const started = new Promise((resolve) => { signingStarted = resolve; });
  const instance = new LabbyBrowserChannel({baseUrl: "http://127.0.0.1:8765", extensionId: "a".repeat(32), browserId: "browser", onError() {}, async onChallenge(challenge, capability) {
    signingStarted();
    await signing;
    await assert.rejects(capability.messageNow("auth.respond", {challenge_id: challenge.challenge_id, signature: "old-signature"}), /channel_disconnected/);
  }});
  instance.connect();
  const oldOpen = sockets[0].onopen();
  instance.receive({version: 1, type: "auth_challenge", request_id: sockets[0].frames[0].request_id, challenge_id: "old-challenge", nonce: "old-nonce"});
  await started;
  instance.connect();
  resumeSigning();
  await oldOpen;
  assert.deepEqual(sockets[1].frames, []);
  assert.equal(sockets[1].closed, undefined);
  instance.close();
});

test("a replaced socket's late failure cannot close or reject its replacement", async () => {
  const sockets = installSocket();
  let rejectOld;
  const oldResync = new Promise((_resolve, reject) => { rejectOld = reject; });
  let readyCalls = 0;
  const instance = new LabbyBrowserChannel({baseUrl: "http://127.0.0.1:8765", extensionId: "a".repeat(32), onChallenge() {}, onReady() { return ++readyCalls === 1 ? oldResync : undefined; }, onError() {}});
  instance.connect();
  const oldOpen = sockets[0].onopen();
  await instance.ready;
  instance.connect();
  const newReady = instance.ready;
  rejectOld(new Error("stale resync"));
  await oldOpen;
  await sockets[1].onopen();
  await newReady;
  assert.equal(sockets[1].closed, undefined);
  instance.close();
});
