import test from "node:test";
import assert from "node:assert/strict";
import {LabbyBrowserChannel} from "../src/channel.js";

function channel(options = {}) {
  const instance = new LabbyBrowserChannel({baseUrl: "http://127.0.0.1:8765", extensionId: "a".repeat(32), ...options});
  const frames = [];
  instance.socket = {readyState: 1, send(value) { frames.push(JSON.parse(value)); }};
  globalThis.WebSocket = {OPEN: 1};
  return {instance, frames};
}

test("sends versioned plain JSON and correlates replies", async () => {
  const {instance, frames} = channel();
  const pending = instance.messageNow("pairing.request", {display_name: "Chrome", public_key: "key"});
  assert.equal(frames[0].version, 1);
  assert.equal(frames[0].type, "pairing_request");
  assert.equal(frames[0].extension_id, "a".repeat(32));
  instance.receive({version: 1, request_id: frames[0].request_id, type: "pairing_pending", pairing_id: "pair", expires_at: 1});
  assert.equal((await pending).payload.pairing_id, "pair");
});

test("maps Rust tool calls to extension events", async () => {
  let event;
  const {instance} = channel({onEvent(value) { event = value; }});
  instance.receive({version: 1, type: "tool_call", call_id: "call", tab_id: 1, document_id: "doc", catalog_revision: 2, tool_name: "search", arguments: {}});
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(event.type, "tool.call");
  assert.equal(event.payload.call_id, "call");
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
