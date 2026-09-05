import assert from "node:assert/strict";
import {webcrypto} from "node:crypto";
import test from "node:test";
import {createIdentityManager} from "../src/identity.js";

class MemoryKeyStore {
  value;
  async get() { return this.value; }
  async set(value) { this.value = structuredClone(value); }
  async clear() { this.value = undefined; }
}

class MemoryStorage {
  constructor(initial = {}) { this.values = {...initial}; }
  async get(keys) {
    const requested = typeof keys === "string" ? [keys] : keys;
    return Object.fromEntries(requested.filter((key) => key in this.values).map((key) => [key, this.values[key]]));
  }
  async remove(keys) { for (const key of typeof keys === "string" ? [keys] : keys) delete this.values[key]; }
}

function manager(keyStore = new MemoryKeyStore(), storage = new MemoryStorage()) {
  return {keyStore, storage, identity: createIdentityManager({keyStore, storage, subtle: webcrypto.subtle})};
}

test("persists a non-extractable Ed25519 CryptoKey across worker restart", async () => {
  const state = manager();
  const first = await state.identity.ensure();
  assert.equal(first.privateKey.extractable, false);
  await assert.rejects(webcrypto.subtle.exportKey("jwk", first.privateKey), /extractable/i);

  const restarted = createIdentityManager({keyStore: state.keyStore, storage: state.storage, subtle: webcrypto.subtle});
  const second = await restarted.ensure();
  assert.equal(second.publicKey, first.publicKey);
  const signature = await restarted.sign(Buffer.from("nonce").toString("base64url"));
  assert.ok(signature.byteLength > 0);
  assert.equal("privateKey" in state.storage.values, false);
});

test("legacy private JWK is removed and forces a new unpaired identity", async () => {
  const storage = new MemoryStorage({privateKey: {kty: "OKP", d: "secret"}, publicKey: "old", browserId: "browser", pairingId: "pair"});
  const state = manager(new MemoryKeyStore(), storage);
  const identity = await state.identity.ensure();
  assert.notEqual(identity.publicKey, "old");
  assert.deepEqual(storage.values, {});
  assert.equal(identity.privateKey.extractable, false);
});

test("corrupt records fail closed, rotate, and clear stale association", async () => {
  const keyStore = new MemoryKeyStore();
  keyStore.value = {version: 1, publicKey: "broken", privateKey: {extractable: false}};
  const storage = new MemoryStorage({browserId: "stale", pairingId: "pending"});
  const state = manager(keyStore, storage);
  const repaired = await state.identity.ensure();
  assert.notEqual(repaired.publicKey, "broken");
  assert.deepEqual(storage.values, {});
});

test("concurrent startup initialization creates one durable identity", async () => {
  const state = manager();
  const [first, second] = await Promise.all([state.identity.ensure(), state.identity.ensure()]);
  assert.equal(first.publicKey, second.publicKey);
  const restarted = createIdentityManager({keyStore: state.keyStore, storage: state.storage, subtle: webcrypto.subtle});
  assert.equal((await restarted.ensure()).publicKey, first.publicKey);
});

test("revocation erases credential and association before re-pair", async () => {
  const state = manager(new MemoryKeyStore(), new MemoryStorage({browserId: "revoked"}));
  const prior = await state.identity.ensure();
  await state.identity.revoke();
  assert.equal(state.keyStore.value, undefined);
  assert.deepEqual(state.storage.values, {});
  const replacement = await state.identity.ensure();
  assert.notEqual(replacement.publicKey, prior.publicKey);
});
