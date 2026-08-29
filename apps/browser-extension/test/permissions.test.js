import test from "node:test";
import assert from "node:assert/strict";
import {BROAD_ORIGINS, disableAllTabs, enableAllTabs, reconcileModeAfterRemoval} from "../src/permissions.js";

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
