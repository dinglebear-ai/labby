import test from "node:test";
import assert from "node:assert/strict";
import {parseLoopbackBaseUrl} from "../src/base_url.js";

test("accepts and normalizes strict loopback Labby endpoints", () => {
  assert.equal(parseLoopbackBaseUrl("http://localhost:8765/"), "http://localhost:8765");
  assert.equal(parseLoopbackBaseUrl("http://127.0.0.1:8765"), "http://127.0.0.1:8765");
  assert.equal(parseLoopbackBaseUrl("https://[::1]:8765"), "https://[::1]:8765");
});

test("rejects remote, deceptive, credentialed, and decorated endpoints", () => {
  for (const value of [
    "https://example.com", "http://localhost.example.com:8765",
    "http://localhost@evil.example:8765", "http://user:pass@localhost:8765",
    "http://127.0.0.2:8765", "http://127.1:8765", "http://2130706433:8765", "http://0.0.0.0:8765",
    "http://localhost:8765/path", "http://localhost:8765?next=evil", "http://localhost:8765/#x",
    "javascript:alert(1)"
  ]) assert.throws(() => parseLoopbackBaseUrl(value), /invalid_base_url/, value);
});
