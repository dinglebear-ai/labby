import test from "node:test";
import assert from "node:assert/strict";
import {parseBaseUrl} from "../src/base_url.js";

test("accepts and normalizes loopback Labby endpoints", () => {
  assert.equal(parseBaseUrl("http://localhost:8765/"), "http://localhost:8765");
  assert.equal(parseBaseUrl("http://127.0.0.1:8765"), "http://127.0.0.1:8765");
  assert.equal(parseBaseUrl("https://[::1]:8765"), "https://[::1]:8765");
});

test("accepts HTTPS remote Labby endpoints", () => {
  assert.equal(parseBaseUrl("https://labby.dinglebear.ai"), "https://labby.dinglebear.ai");
  assert.equal(parseBaseUrl("https://labby.example:9443/"), "https://labby.example:9443");
});

test("rejects insecure remote, credentialed, and decorated endpoints", () => {
  for (const value of [
    "http://example.com", "http://localhost.example.com:8765",
    "http://127.0.0.2:8765", "http://127.1:8765", "http://2130706433:8765", "http://0.0.0.0:8765",
    "https://localhost@evil.example:8765", "https://user:pass@example.com",
    "https://labby.example/path", "https://labby.example?next=evil", "https://labby.example/#x",
    "javascript:alert(1)"
  ]) assert.throws(() => parseBaseUrl(value), /invalid_base_url/, value);
});
