import test from "node:test";
import assert from "node:assert/strict";
import {BROAD_ORIGINS, disableAllTabs, enableAllTabs, ensureLabbyOriginPermission, hasLabbyOriginPermission, labbyOriginPermission, reconcileModeAfterRemoval} from "../src/permissions.js";

test("Labby host permission strips ports and preserves IPv6 literals", () => {
  assert.equal(labbyOriginPermission("https://labby.example.com:8443"), "https://labby.example.com/*");
  assert.equal(labbyOriginPermission("http://127.0.0.1:8765"), "http://127.0.0.1/*");
  assert.equal(labbyOriginPermission("http://[::1]:8765"), "http://[::1]/*");
});

test("Labby host permission check uses the same exact pattern", async () => {
  let checked;
  const granted = await hasLabbyOriginPermission({contains: async (value) => { checked = value; return false; }}, "https://labby.example.com:8443");
  assert.equal(granted, false);
  assert.deepEqual(checked, {origins: ["https://labby.example.com/*"]});
});

test("Labby host permission reuses an existing exact grant", async () => {
  let requested = false;
  const granted = await ensureLabbyOriginPermission({
    contains: async (value) => { assert.deepEqual(value, {origins: ["https://labby.example.com/*"]}); return true; },
    request: async () => { requested = true; return true; }
  }, "https://labby.example.com:8443");
  assert.equal(granted, true);
  assert.equal(requested, false);
});

test("Labby host permission requests only the exact configured host", async () => {
  let requested;
  const granted = await ensureLabbyOriginPermission({
    contains: async () => false,
    request: async (value) => { requested = value; return true; }
  }, "https://labby.example.com:8443");
  assert.equal(granted, true);
  assert.deepEqual(requested, {origins: ["https://labby.example.com/*"]});
});

test("all-tabs enablement requests both broad optional origins", async () => {
  let requested;
  const granted = await enableAllTabs({request: async (value) => { requested = value; return true; }});
  assert.equal(granted, true);
  assert.deepEqual(requested, {origins: BROAD_ORIGINS});
});

test("returning to granted-sites reports a broad permission that remains held", async () => {
  const result = await disableAllTabs({
    remove: async () => false,
    contains: async () => true
  });
  assert.deepEqual(result, {removed: false, stillBroad: true});
});

test("external broad-permission revocation persists granted-sites mode", async () => {
  let saved;
  const mode = await reconcileModeAfterRemoval(
    {contains: async () => false},
    {get: async () => ({scanningMode: "all_tabs"}), set: async (value) => { saved = value; }}
  );
  assert.equal(mode, "granted_sites");
  assert.deepEqual(saved, {scanningMode: "granted_sites"});
});
